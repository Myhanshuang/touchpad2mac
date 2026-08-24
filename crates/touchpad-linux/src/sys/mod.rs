//! The mockable OS syscall/filesystem seam (M4).
//!
//! Every real operating-system interaction of the Linux device boundary goes
//! through the [`Sys`] trait: directory enumeration, `open`/`close`, `read`
//! of raw event bytes, and the evdev `ioctl` queries (`EVIOCGNAME`,
//! `EVIOCGID`, `EVIOCGBIT`, `EVIOCGPROP`, `EVIOCGABS`, `EVIOCGMTSLOTS`,
//! `EVIOCGKEY`, `EVIOCSCLOCKID`, `EVIOCGRAB`).
//!
//! * The real Linux implementation is [`ffi::LinuxSys`] (Linux-only, and the
//!   single module in this crate that contains `unsafe`).
//! * [`mock::MockSys`] is the programmable test double used by every test, so
//!   the test suite never opens or grabs a real device and runs identically
//!   on any platform.
//!
//! All higher-level M4 modules (device probing, the grab guard, the
//! `SYN_DROPPED` snapshot adapter, the input runtime) are written against
//! [`Sys`] only, which is what makes them fully mock-testable.
//!
//! This module (and its [`requests`] and [`mock`] submodules) is `unsafe`-free;
//! only the Linux-only [`ffi`] submodule contains `unsafe`, and every block there
//! documents its safety invariants individually.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(target_os = "linux")]
pub mod ffi;
pub mod mock;
pub mod requests;

/// Opaque handle to an open device, produced by [`Sys::open`].
///
/// `Fd` is a plain integer token (`Copy`): on the real Linux path it indexes
/// the fd registry inside [`ffi::LinuxSys`] (which owns the actual
/// `OwnedFd`), and in tests it is an arbitrary id handed out by
/// [`mock::MockSys`]. Ownership of the underlying descriptor always stays
/// with the [`Sys`] implementation, which is what makes [`Sys::close`]
/// naturally idempotent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fd(u64);

impl Fd {
    /// Creates an opaque handle with the given id.
    #[must_use]
    pub(crate) const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The opaque id.
    #[must_use]
    pub(crate) const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Structured system-call failure, kept actionable for callers (M4
/// requirement 7: no `/dev/input`, permission denied, EINTR, ... must surface
/// as explainable errors, never a panic).
#[derive(Debug, thiserror::Error)]
pub enum SysError {
    /// A raw I/O error (with errno) that did not map to a more specific
    /// variant below.
    #[error("I/O error: {0}")]
    Io(io::Error),
    /// `ENOENT`: the path does not exist (e.g. no `/dev/input` directory, or
    /// a device node that vanished).
    #[error("no such file or directory: {path}")]
    NotFound {
        /// The path that does not exist.
        path: PathBuf,
    },
    /// `EACCES`/`EPERM`: the caller may not access the path.
    #[error("permission denied for {path}: {source}")]
    PermissionDenied {
        /// The path that was denied.
        path: PathBuf,
        /// The underlying OS error.
        source: io::Error,
    },
    /// `EINTR`: the syscall was interrupted by a signal. M4 surfaces this
    /// rather than retrying invisibly; M5's signal handling maps it to a
    /// graceful shutdown.
    #[error("operation interrupted by a signal (EINTR)")]
    Interrupted,
    /// The operation targeted a handle that has already been closed.
    #[error("operation on closed device handle {0:?}")]
    Closed(Fd),
    /// The call was structurally invalid (e.g. a zero-length ioctl buffer).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A response was truncated relative to the data the caller consumes:
    /// e.g. `EVIOCGKEY` returned fewer bytes than needed to cover the
    /// physical buttons, or a capability bit array was shorter than the full
    /// kernel array. The caller fails closed rather than treating the
    /// missing bytes as zeroes (M4 review R7).
    #[error("truncated response from {operation}: got {returned} bytes, need at least {required} for a complete result")]
    TruncatedResponse {
        /// Which query was truncated (e.g. `"EVIOCGKEY"`).
        operation: &'static str,
        /// The number of bytes the device actually provided.
        returned: usize,
        /// The minimum number of bytes the caller needs to consume its fields.
        required: usize,
    },
}

/// `CLOCK_MONOTONIC` (`linux/time.h`): the clock the runtime requires for
/// `input_event` timestamps.
///
/// The evdev client clock is zero-initialized to `INPUT_CLK_REAL`
/// (`CLOCK_REALTIME`, value 0); the kernel switches it to this clock only
/// after `EVIOCSCLOCKID(CLOCK_MONOTONIC)` succeeds (M4 review R1).
pub const CLOCK_MONOTONIC: u32 = 1;

impl SysError {
    /// Maps a raw `io::Error` (from `last_os_error`) onto the structured
    /// variants, attaching `path` for the actionable `ENOENT`/`EACCES` cases.
    ///
    /// Linux-only: the errno values it distinguishes are Linux ABI. The
    /// portable seam keeps it private so only the real [`ffi::LinuxSys`]
    /// (and Linux-only tests) use it.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub(crate) fn from_errno(path: &Path, error: io::Error) -> Self {
        match error.raw_os_error() {
            Some(libc::ENOENT) => SysError::NotFound {
                path: path.to_path_buf(),
            },
            Some(libc::EACCES) | Some(libc::EPERM) => SysError::PermissionDenied {
                path: path.to_path_buf(),
                source: error,
            },
            Some(libc::EINTR) => SysError::Interrupted,
            _ => SysError::Io(error),
        }
    }
}

/// Device identity as returned by `EVIOCGID` (`struct input_id`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputId {
    /// `input_id.bustype` (e.g. `BUS_I2C`, `BUS_USB`, `BUS_HOST`).
    pub bustype: u16,
    /// `input_id.vendor`.
    pub vendor: u16,
    /// `input_id.product`.
    pub product: u16,
    /// `input_id.version`.
    pub version: u16,
}

/// One axis description as returned by `EVIOCGABS` (`struct input_absinfo`).
///
/// This is the raw kernel report; conversion to the core
/// [`touchpad_core::AxisInfo`] (which stores an optional non-zero
/// resolution) happens in the device probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AbsInfo {
    /// Current axis value (`input_absinfo.value`).
    pub value: i32,
    /// Minimum raw value (`input_absinfo.minimum`).
    pub min: i32,
    /// Maximum raw value (`input_absinfo.maximum`).
    pub max: i32,
    /// Value noise threshold (`input_absinfo.fuzz`).
    pub fuzz: i32,
    /// Dead zone around center (`input_absinfo.flat`).
    pub flat: i32,
    /// Resolution in units per millimeter; `0` means the device reports none.
    pub resolution: i32,
}

/// The mockable OS seam: every filesystem operation and syscall used by the
/// Linux device boundary.
///
/// Implementations are expected to be cheap and side-effect recording; all
/// methods take `&self` so a single implementation can be shared (e.g. by an
/// [`std::rc::Rc`]) between the read loop, the grab guard, and the resync snapshot
/// adapter. The real [`ffi::LinuxSys`] is stateless apart from its fd
/// registry; [`mock::MockSys`] uses interior mutability to record calls.
pub trait Sys {
    /// Lists the entries of `path` (e.g. `/dev/input`).
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, SysError>;

    /// Opens `path` (read-only) and returns a handle.
    fn open(&self, path: &Path) -> Result<Fd, SysError>;

    /// Closes `path`'s handle. Idempotent: closing an already-closed handle
    /// succeeds.
    fn close(&self, fd: Fd) -> Result<(), SysError>;

    /// Reads raw bytes into `buf`. Returns the number of bytes read, or `0`
    /// at end of stream (device unplugged). `EINTR` surfaces as
    /// [`SysError::Interrupted`].
    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError>;

    /// `EVIOCGRAB`: grabs (`grab == true`) or releases (`false`) the device.
    fn ioctl_grab(&self, fd: Fd, grab: bool) -> Result<(), SysError>;

    /// `EVIOCSCLOCKID(clock_id)`: selects the clock used for `input_event`
    /// timestamps on this fd. The kernel defaults to `INPUT_CLK_REAL`
    /// (`CLOCK_REALTIME`), so the runtime must issue
    /// `EVIOCSCLOCKID(CLOCK_MONOTONIC)` on its session fd before grab and
    /// before reading any events (M4 review R1).
    fn ioctl_set_clock_id(&self, fd: Fd, clock_id: u32) -> Result<(), SysError>;

    /// `EVIOCGNAME(len)`: copies the device name into `buf` (NUL-terminated
    /// by the kernel) and returns the number of bytes copied.
    fn ioctl_name(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError>;

    /// `EVIOCGID`: returns the device identity.
    fn ioctl_id(&self, fd: Fd) -> Result<InputId, SysError>;

    /// `EVIOCGBIT(ev, len)`: copies the bit array for event type `ev`
    /// (`EV_KEY`, `EV_ABS`, or `0` for the `evbit` array itself) into `buf`
    /// and returns the number of bytes copied.
    fn ioctl_ev_bits(&self, fd: Fd, ev_type: u16, buf: &mut [u8]) -> Result<usize, SysError>;

    /// `EVIOCGPROP(len)`: copies the `INPUT_PROP_*` bit array into `buf` and
    /// returns the number of bytes copied.
    fn ioctl_prop_bits(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError>;

    /// `EVIOCGKEY(len)`: copies the global key state bit array into `buf`
    /// and returns the number of bytes actually copied into `buf`.
    ///
    /// The kernel routes `EVIOCGKEY` through `evdev_handle_get_val` →
    /// `bits_to_user`, which copies `min(len, BITS_TO_BYTES(KEY_MAX))`
    /// bytes and **returns that copied byte count** as the ioctl result.
    /// It does *not* return 0 on success — only `EVIOCGMTSLOTS`
    /// (`evdev_handle_mt_request`) has zero-on-success semantics. The
    /// caller validates that the returned length covers every bit it
    /// consumes (M4 review R7/RR1).
    fn ioctl_key_state(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError>;

    /// `EVIOCGABS(abs)`: returns the axis description for `abs`.
    fn ioctl_absinfo(&self, fd: Fd, abs_code: u16) -> Result<AbsInfo, SysError>;

    /// `EVIOCGMTSLOTS(len)`: `buf[0]` carries the `ABS_MT_*` code to query
    /// and is overwritten; `buf[1..]` receives one value per slot.
    ///
    /// The kernel returns **0 on success** (`evdev_handle_mt_request`), not
    /// a byte count, so the seam models success/failure only. The number of
    /// values written is implied by the device's own slot count, which the
    /// caller must derive from `ABS_MT_SLOT` on the same fd and bound with
    /// `MAX_SLOT_COUNT`; a successful call with a `(slot_count + 1)`-element
    /// buffer therefore fully populates `buf[1..=slot_count]` (M4 review
    /// R2).
    fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError>;

    /// Polls `fd` for readability with a bounded timeout (M10 takeover loop:
    /// the loop wakes at a short fixed quantum, checks the injected clock /
    /// stop / fault, and reads only when this returns `true`).
    ///
    /// Revents are classified explicitly (M10 review R2): `POLLIN`,
    /// `POLLHUP`, and `POLLERR` return `Ok(true)` — a read on `fd` would
    /// make progress (data, or the real EOF/error of an unplugged/failed
    /// device) — `POLLNVAL` returns an immediate structured error
    /// ([`SysError::InvalidArgument`]: the fd is invalid, never idle), and a
    /// pure timeout returns `Ok(false)` (idle). `EINTR` is surfaced as
    /// [`SysError::Interrupted`] (the caller decides whether a stop was
    /// requested).
    fn poll(&self, fd: Fd, timeout: Duration) -> Result<bool, SysError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_ids_are_distinct_and_copyable() {
        let a = Fd::new(1);
        let b = Fd::new(2);
        let copy = a;
        assert_eq!(copy, a);
        assert_ne!(a, b);
        assert_eq!(a.as_u64(), 1);
    }

    #[test]
    fn input_id_defaults_to_zeros() {
        assert_eq!(
            InputId::default(),
            InputId {
                bustype: 0,
                vendor: 0,
                product: 0,
                version: 0,
            }
        );
    }

    #[test]
    fn sys_error_messages_are_actionable() {
        let not_found = SysError::NotFound {
            path: PathBuf::from("/dev/input"),
        };
        assert!(not_found.to_string().contains("/dev/input"));
        assert!(SysError::Interrupted.to_string().contains("EINTR"));
        assert!(SysError::Closed(Fd::new(3))
            .to_string()
            .contains("closed device handle"));
        let truncated = SysError::TruncatedResponse {
            operation: "EVIOCGKEY",
            returned: 20,
            required: 35,
        };
        assert!(truncated.to_string().contains("EVIOCGKEY"), "{truncated}");
    }

    #[test]
    fn clock_monotonic_is_the_kernel_constant() {
        // linux/time.h: CLOCK_REALTIME == 0, CLOCK_MONOTONIC == 1.
        assert_eq!(CLOCK_MONOTONIC, 1);
    }
}
