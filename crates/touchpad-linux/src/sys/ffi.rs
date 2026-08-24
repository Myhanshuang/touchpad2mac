//! The real Linux implementation of the [`Sys`] seam (M4).
//!
//! [`LinuxSys`] is the only module in this crate that uses `unsafe`, and it
//! is compiled only on `target_os = "linux"`. Every `unsafe` block sits on
//! the direct FFI boundary — `open(2)`, `read(2)`, `ioctl(2)`, and the
//! ownership transfer of a freshly opened descriptor into an
//! [`OwnedFd`] — with its safety invariants documented at the call site.
//!
//! ## Live Linux target restriction (M4 review RR3)
//!
//! The live Linux boundary (this FFI adapter plus the [`crate::event`]
//! `input_event` decoder) is implemented and verified only for **x86_64
//! Linux**: other Linux ABIs (32-bit `timeval`/time64 fields, sparc64
//! `usec`+padding, non-asm-generic ioctl encodings) are not supported and
//! fail at compile time via [`crate::event`]'s `compile_error!` (gated on
//! `target_os = "linux"` non-`x86_64`). Non-Linux offline replay/mock code
//! remains portable.
//!
//! ## Fd registry
//!
//! The concrete type owns every open descriptor in a registry
//! (`Vec<Option<OwnedFd>>`) and hands out opaque [`Fd`] index tokens.
//! Dropping the `OwnedFd` (on [`Sys::close`], or when the registry itself is
//! dropped) closes the descriptor exactly once, which makes [`Sys::close`]
//! naturally idempotent and prevents double-close on any error path.
//!
//! ## Raw fds and the clock domain
//!
//! The evdev client clock is zero-initialized to `INPUT_CLK_REAL`
//! (`CLOCK_REALTIME`, value 0); the kernel switches it to `CLOCK_MONOTONIC`
//! only after `EVIOCSCLOCKID(CLOCK_MONOTONIC)` succeeds
//! (`drivers/input/evdev.c::evdev_set_clk_type`). Timestamps read here are
//! therefore **monotonic only because the runtime explicitly requests it**
//! on its session fd (via [`Sys::ioctl_set_clock_id`], before grab and
//! before any read). The conversion layer ([`crate::event`]) treats the
//! `timeval` values as monotonic and never as wall-clock time.

#![allow(unsafe_code)]
#![warn(missing_docs)]

use std::cell::RefCell;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::time::Duration;

use super::requests;
use super::{AbsInfo, Fd, InputId, Sys, SysError};

/// The real Linux syscall seam: `open`/`read`/`ioctl`/`close` through
/// `libc`, with every descriptor owned in an internal registry.
///
/// All methods are `&self` (the registry uses interior mutability), so one
/// instance can be shared by the read loop, the grab guard, and the resync
/// snapshot adapter.
#[derive(Default)]
pub struct LinuxSys {
    /// Owned descriptors; index `i` corresponds to [`Fd`] `i`. `None` marks
    /// a closed slot.
    registry: RefCell<Vec<Option<OwnedFd>>>,
}

impl LinuxSys {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves an opaque [`Fd`] to its raw descriptor, or fails when it was
    /// already closed.
    fn raw_fd(&self, fd: Fd) -> Result<RawFd, SysError> {
        let registry = self.registry.borrow();
        let index = fd.as_u64() as usize;
        match registry.get(index).and_then(Option::as_ref) {
            Some(owned) => Ok(owned.as_raw_fd()),
            None => Err(SysError::Closed(fd)),
        }
    }
}

impl Sys for LinuxSys {
    fn read_dir(&self, path: &Path) -> Result<Vec<std::path::PathBuf>, SysError> {
        let entries = std::fs::read_dir(path).map_err(|error| SysError::from_errno(path, error))?;
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(SysError::Io)?;
            out.push(entry.path());
        }
        Ok(out)
    }

    fn open(&self, path: &Path) -> Result<Fd, SysError> {
        // `CString::new` rejects interior NUL bytes, so `c_path` is a valid
        // NUL-terminated C string for `open(2)`.
        let c_path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
            SysError::InvalidArgument(format!(
                "path contains an interior NUL byte: {}",
                path.display()
            ))
        })?;
        // SAFETY: `c_path.as_ptr()` is a valid NUL-terminated path string and
        // the flags are valid `open(2)` flags (`O_RDONLY | O_CLOEXEC`).
        let raw = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if raw < 0 {
            return Err(SysError::from_errno(path, io::Error::last_os_error()));
        }
        // SAFETY: `raw` is a freshly opened descriptor (open(2) returned
        // >= 0) that this process has never duplicated or otherwise aliased,
        // so transferring ownership into an `OwnedFd` is valid.
        let owned = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut registry = self.registry.borrow_mut();
        let index = registry.len() as u64;
        registry.push(Some(owned));
        Ok(Fd::new(index))
    }

    fn close(&self, fd: Fd) -> Result<(), SysError> {
        let mut registry = self.registry.borrow_mut();
        let index = fd.as_u64() as usize;
        match registry.get_mut(index) {
            // Replacing the slot with `None` drops the `OwnedFd`, closing the
            // descriptor exactly once (safe API — no `unsafe` here).
            Some(slot) if slot.is_some() => {
                *slot = None;
                Ok(())
            }
            // Already closed (or never opened): idempotent success.
            _ => Ok(()),
        }
    }

    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "read buffer is empty; a zero-length read cannot be distinguished from EOF"
                    .to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `raw` is a valid open descriptor (registry) and `buf` is a
        // writable slice of `buf.len()` bytes; `read(2)` writes at most
        // `buf.len()` bytes. `EINTR` is surfaced as
        // [`SysError::Interrupted`] rather than retried, so M5's signal
        // handling can observe it.
        let n = unsafe { libc::read(raw, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                return Err(SysError::Interrupted);
            }
            return Err(SysError::Io(error));
        }
        Ok(n as usize)
    }

    fn ioctl_set_clock_id(&self, fd: Fd, clock_id: u32) -> Result<(), SysError> {
        let raw = self.raw_fd(fd)?;
        let clock: libc::c_uint = clock_id;
        // SAFETY: `clock` is a valid `c_uint` of exactly 4 bytes (the size
        // encoded in `EVIOCSCLOCKID`); for this `_IOW` request the kernel
        // only reads one `__u32` from it (`get_user` in
        // `evdev_do_ioctl`), never writes.
        unsafe {
            ioctl_call(
                raw,
                requests::eviocsc_lockid(),
                (&clock as *const libc::c_uint).cast_mut().cast(),
            )
        }?;
        Ok(())
    }

    fn ioctl_grab(&self, fd: Fd, grab: bool) -> Result<(), SysError> {
        let raw = self.raw_fd(fd)?;
        let one: libc::c_int = 1;
        // EVIOCGRAB takes a pointer-to-int argument; the kernel only tests
        // whether the pointer is non-null (grab) or null (release).
        let arg: *mut libc::c_void = if grab {
            (&one as *const libc::c_int).cast_mut().cast()
        } else {
            std::ptr::null_mut()
        };
        // SAFETY: for a grab, `arg` points to a valid `c_int` (the kernel
        // never dereferences it, only tests non-null); for a release it is
        // NULL. The request's encoded payload size is `sizeof(int)`.
        unsafe { ioctl_call(raw, requests::eviocgrab(), arg) }?;
        Ok(())
    }

    fn ioctl_name(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "EVIOCGNAME buffer is empty".to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `buf` is a writable slice of `buf.len()` bytes and the
        // request encodes exactly that size; `EVIOCGNAME` copies at most
        // `min(len, name length)` bytes and NUL-terminates within `buf`.
        let n = unsafe {
            ioctl_call(
                raw,
                requests::eviocgname(buf.len()),
                buf.as_mut_ptr().cast(),
            )
        }?;
        Ok(n as usize)
    }

    fn ioctl_id(&self, fd: Fd) -> Result<InputId, SysError> {
        let raw = self.raw_fd(fd)?;
        let mut id = libc::input_id {
            bustype: 0,
            vendor: 0,
            product: 0,
            version: 0,
        };
        // SAFETY: `id` is a valid `libc::input_id` of exactly 8 bytes (the
        // size encoded in `EVIOCGID`); the kernel fills all four fields.
        unsafe {
            ioctl_call(
                raw,
                requests::eviocgid(),
                (&mut id as *mut libc::input_id).cast(),
            )
        }?;
        Ok(InputId {
            bustype: id.bustype,
            vendor: id.vendor,
            product: id.product,
            version: id.version,
        })
    }

    fn ioctl_ev_bits(&self, fd: Fd, ev_type: u16, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "EVIOCGBIT buffer is empty".to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `buf` is a writable slice of `buf.len()` bytes and the
        // request encodes exactly that size; `EVIOCGBIT` copies at most
        // `min(len, BITS_TO_BYTES(max_bit))` bytes.
        let n = unsafe {
            ioctl_call(
                raw,
                requests::eviocgbit(ev_type, buf.len()),
                buf.as_mut_ptr().cast(),
            )
        }?;
        Ok(n as usize)
    }

    fn ioctl_prop_bits(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "EVIOCGPROP buffer is empty".to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `buf` is a writable slice of `buf.len()` bytes and the
        // request encodes exactly that size; `EVIOCGPROP` copies at most
        // `min(len, BITS_TO_BYTES(INPUT_PROP_MAX))` bytes.
        let n = unsafe {
            ioctl_call(
                raw,
                requests::eviocgprop(buf.len()),
                buf.as_mut_ptr().cast(),
            )
        }?;
        Ok(n as usize)
    }

    fn ioctl_key_state(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "EVIOCGKEY buffer is empty".to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `buf` is a writable slice of `buf.len()` bytes and the
        // request encodes exactly that size. `EVIOCGKEY` is routed through
        // `evdev_handle_get_val` → `bits_to_user`, which copies
        // `min(len, BITS_TO_BYTES(KEY_MAX))` bytes and **returns the number
        // of bytes copied** (never 0 on success; only `EVIOCGMTSLOTS` uses
        // `evdev_handle_mt_request` and returns 0). The caller validates
        // that the returned length covers every bit it consumes (M4 review
        // R7/RR1).
        let n =
            unsafe { ioctl_call(raw, requests::eviocgkey(buf.len()), buf.as_mut_ptr().cast())? };
        Ok(n as usize)
    }

    fn ioctl_absinfo(&self, fd: Fd, abs_code: u16) -> Result<AbsInfo, SysError> {
        let raw = self.raw_fd(fd)?;
        let mut info = libc::input_absinfo {
            value: 0,
            minimum: 0,
            maximum: 0,
            fuzz: 0,
            flat: 0,
            resolution: 0,
        };
        // SAFETY: `info` is a valid `libc::input_absinfo` of exactly 24 bytes
        // (the size encoded in `EVIOCGABS`); the kernel fills all six fields.
        unsafe {
            ioctl_call(
                raw,
                requests::eviocgabs(abs_code),
                (&mut info as *mut libc::input_absinfo).cast(),
            )
        }?;
        Ok(AbsInfo {
            value: info.value,
            min: info.minimum,
            max: info.maximum,
            fuzz: info.fuzz,
            flat: info.flat,
            resolution: info.resolution,
        })
    }

    fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError> {
        let len = buf
            .len()
            .checked_mul(4)
            .ok_or_else(|| SysError::InvalidArgument("MT slot buffer size overflow".to_string()))?;
        if len == 0 {
            return Err(SysError::InvalidArgument(
                "MT slot buffer is empty".to_string(),
            ));
        }
        let raw = self.raw_fd(fd)?;
        // SAFETY: `buf` is a writable slice of exactly `len` bytes and the
        // request encodes exactly that size. On entry `buf[0]` must carry the
        // `ABS_MT_*` code to query; the kernel writes one value per slot
        // after the leading code and returns 0 on success
        // (`evdev_handle_mt_request`). The snapshot adapter sizes the buffer
        // as `slot_count + 1` elements from the device's own `ABS_MT_SLOT`
        // read on the same fd, so a successful call fully populates
        // `buf[1..=slot_count]` (see [`crate::snapshot`] and M4 review R2).
        unsafe { ioctl_call(raw, requests::eviocgmt_slots(len), buf.as_mut_ptr().cast()) }?;
        Ok(())
    }

    fn poll(&self, fd: Fd, timeout: Duration) -> Result<bool, SysError> {
        let raw = self.raw_fd(fd)?;
        let mut pfd = libc::pollfd {
            fd: raw,
            events: libc::POLLIN,
            revents: 0,
        };
        // Bounded timeout: `poll(2)` takes a non-negative `int` milliseconds.
        let ms = i32::try_from(timeout.as_millis().min(i32::MAX as u128)).unwrap_or(i32::MAX);
        // SAFETY: `raw` is a valid open descriptor (registry) and `pfd` is a
        // fully initialized single-element pollfd array; `poll(2)` writes at
        // most `revents` in the one element and the timeout is a non-negative
        // `int`. `EINTR` is surfaced as [`SysError::Interrupted`] rather than
        // retried, matching the read seam's signal policy.
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                return Err(SysError::Interrupted);
            }
            return Err(SysError::Io(error));
        }
        classify_revents(pfd.revents)
    }
}

/// Classifies `poll(2)` revents (M10 review R2). The real implementation
/// returns ready only for `POLLIN`, so an unplugged/failed evdev fd that
/// wakes with `POLLHUP`/`POLLERR` (without `POLLIN`) was previously
/// converted to idle and the takeover loop repeated until the deadline
/// instead of immediately reading/surfacing the EOF or failure; `POLLNVAL`
/// was likewise treated as idle.
///
/// The classification is explicit:
///
/// * `POLLIN`/`POLLHUP`/`POLLERR` → `Ok(true)`: a read on the fd would make
///   progress — data is available, the peer hung up (an unplugged evdev
///   device reads EOF → `DeviceGone`), or an error condition is pending that
///   the read surfaces — so the bounded takeover loop must read immediately.
/// * `POLLNVAL` → an immediate structured error: the fd is invalid, so no
///   read can ever make progress (fail closed, never idle).
/// * anything else (a pure timeout) → `Ok(false)`: idle; the loop re-checks
///   the injected clock (deadline), the stop, and the bridge fault.
///
/// Pure function so every flag and combination is unit-tested
/// deterministically without a real fd.
fn classify_revents(revents: i16) -> Result<bool, SysError> {
    if revents & libc::POLLNVAL != 0 {
        return Err(SysError::InvalidArgument(
            "poll: the fd is invalid (POLLNVAL); no read can make progress".to_string(),
        ));
    }
    Ok(revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0)
}

/// Runs one evdev ioctl and maps `EINTR`/`errno` to [`SysError`].
///
/// # Safety
///
/// `arg` must be valid for `request`'s direction and encoded payload size:
/// for `_IOC_READ` requests it must point to a writable buffer of at least
/// the encoded size; for `EVIOCGRAB` it must be `NULL` (release) or point to
/// a valid `c_int` (grab). The kernel writes at most the encoded size.
unsafe fn ioctl_call(
    raw: RawFd,
    request: u32,
    arg: *mut std::ffi::c_void,
) -> Result<isize, SysError> {
    // SAFETY: guaranteed by the caller (see above); `raw` is a valid open
    // descriptor (registry).
    let rc = unsafe { libc::ioctl(raw, libc::c_ulong::from(request), arg) };
    if rc < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            return Err(SysError::Interrupted);
        }
        return Err(SysError::Io(error));
    }
    Ok(rc as isize)
}

// ---------------------------------------------------------------------------
// M5: SIGINT/SIGTERM handler for the controlled record stop.
//
// ## Memory safety (M5 re-review R1) — no caller-owned storage on the
// async handler path
//
// The handler's **only** side effect is one lock-free relaxed atomic store
// into the process-lifetime static [`TERMINATION_REQUESTED`]. It never loads
// or dereferences a caller-owned pointer, so no teardown interleaving can
// ever leave an in-flight handler touching freed memory: the handler's
// target is `'static` storage that is never reclaimed. In particular,
// restoring a disposition does not wait for a handler that already started,
// but that is irrelevant to memory safety here — such a handler's remaining
// work is a store into process-lifetime memory, which stays valid no matter
// when it resumes. The previous design's race (an in-flight handler
// dereferencing a freed `Arc<AtomicBool>` after guard teardown released the
// last clone) is eliminated by construction: there is no caller allocation
// on the async handler path to reclaim.
//
// Guard teardown restores the previous `SIGINT`/`SIGTERM` dispositions
// first (after that, our handler can no longer run for either signal),
// resets [`TERMINATION_REQUESTED`] (a signal that arrived before restoration
// was already handled by our handler; one that arrives after restoration is
// handled by the restored disposition), and only then clears the
// single-install marker. Because the handler dereferences no caller-owned
// memory, the drop ordering is a correctness/cleanliness choice, not a
// memory-safety synchronization.
//
// ## Single active install (M5 review R1, kept)
//
// A process-global [`INSTALLED`] flag enforces the documented "only one
// handler" invariant in code: a second [`install_termination_handler`] while
// a guard is alive fails with a structured
// [`TerminationInstallError::AlreadyInstalled`] instead of installing a
// second handler over the first. This keeps restoration order deterministic:
// the guard that is dropped always restores the dispositions captured by the
// *one* active install.
//
// ## Concurrency boundary (documented honestly)
//
// Installation and removal (guard construction/drop) are **single-threaded**
// — the CLI installs once on its main thread (M4 §7) and drops the guard at
// process end. Signal *delivery* may interrupt any thread at any point; the
// handler body is async-signal-safe (one lock-free relaxed atomic store, no
// allocation, no locks). The single-threaded install/remove convention
// exists for deterministic disposition-restoration ordering, **not** for
// memory safety: even a guard drop racing an already-running handler cannot
// produce undefined behavior, because the handler's target is process-
// lifetime static storage. `INSTALLED` and `TERMINATION_REQUESTED` are
// touched only by the installing thread / guard drop / the async handler.
//
// The handler is installed without `SA_RESTART`, so a pending signal
// interrupts a blocking `read(2)`; the runtime maps "EINTR + stop requested"
// to a graceful stop (M5).
// ---------------------------------------------------------------------------

/// Process-lifetime stop state set by the termination handler (M5 re-review
/// R1). The handler's only side effect is a store here. This `'static`
/// storage is never reclaimed, so no guard teardown can leave an in-flight
/// handler with a dangling target.
static TERMINATION_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether a [`TerminationHandlerGuard`] is currently installed (single
/// active install, M5 review R1). Set before the first `sigaction` and
/// cleared after both dispositions are restored and `TERMINATION_REQUESTED`
/// is reset.
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The async-signal-safe termination handler: records a stop request in the
/// process-lifetime static.
///
/// Runs on an arbitrary thread at an arbitrary point of the process. The
/// body is one lock-free atomic store (async-signal-safe: no allocation, no
/// locks, no caller-owned memory).
extern "C" fn termination_handler(_signal: libc::c_int) {
    TERMINATION_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the termination handler has been invoked since the last install
/// or guard drop (M5 re-review R1).
///
/// Reads the process-lifetime static; safe to call from any thread at any
/// time, with or without a handler installed. The runtime and the record
/// command consult this (together with any attached stop flag) to turn an
/// interrupted read into a graceful stop.
#[must_use]
pub(crate) fn termination_requested() -> bool {
    TERMINATION_REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Test-only: resets the process-lifetime stop static (test isolation for
/// the model test that deliberately fires the handler after teardown).
#[cfg(test)]
pub(crate) fn reset_termination_requested_for_test() {
    TERMINATION_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Failure of [`install_termination_handler`].
#[derive(Debug, thiserror::Error)]
pub enum TerminationInstallError {
    /// Another termination handler is already installed (the single-active
    /// install invariant, M5 review R1).
    #[error("a SIGINT/SIGTERM termination handler is already installed; only one may be active at a time (drop the first guard before installing another)")]
    AlreadyInstalled,
    /// `sigaction(2)` failed.
    #[error("could not install the SIGINT/SIGTERM signal handler: {0}")]
    Install(#[from] io::Error),
}

/// Restores the previous `SIGINT`/`SIGTERM` dispositions on drop, resets the
/// process-lifetime stop static, and clears the single-install marker.
///
/// The handler dereferences no caller-owned memory (its only store targets
/// the process-lifetime [`TERMINATION_REQUESTED`] static), so this guard
/// owns nothing on the caller's behalf — dropping it cannot race an
/// in-flight handler over a freed allocation (M5 re-review R1).
pub struct TerminationHandlerGuard {
    previous_sigint: libc::sigaction,
    previous_sigterm: libc::sigaction,
    restored: bool,
}

impl Drop for TerminationHandlerGuard {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        // First restore both dispositions: after that our handler can no
        // longer run for either signal. Then reset the process-lifetime stop
        // static (a stop request that arrived before restoration was already
        // recorded by the handler; one that arrives after restoration is
        // handled by the restored disposition) and release the
        // single-install marker. No caller-owned allocation is involved, so
        // this ordering is a cleanliness choice, not a memory-safety
        // synchronization: an in-flight handler that resumes after teardown
        // stores into the same never-reclaimed static.
        // SAFETY: restore the exact dispositions captured at install time.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous_sigint, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &self.previous_sigterm, std::ptr::null_mut());
        }
        TERMINATION_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
        INSTALLED.store(false, std::sync::atomic::Ordering::SeqCst);
        self.restored = true;
    }
}

/// Installs the [`termination_handler`] for `SIGINT` and `SIGTERM`, which
/// records a stop request in the process-lifetime static
/// [`TERMINATION_REQUESTED`].
///
/// The handler is installed without `SA_RESTART` so a pending signal
/// interrupts a blocking `read(2)` (which the [`Sys`] seam surfaces as
/// [`SysError::Interrupted`]).
///
/// The handler dereferences **no caller-owned memory** (M5 re-review R1):
/// its only side effect is a store into the process-lifetime static, so the
/// safe API cannot be made to dangle by dropping any caller allocation, and
/// a guard teardown racing an in-flight handler is memory-safe by
/// construction. Callers observe the stop request through
/// [`termination_requested`] (or the runtime/record command's stop handling).
///
/// Only **one** handler may be installed at a time (M5 review R1): a second
/// call while a guard is alive fails with
/// [`TerminationInstallError::AlreadyInstalled`] instead of installing a
/// second handler over the first. After the first guard is dropped, a fresh
/// install succeeds (and starts from a clean stop state). On a partial
/// install failure (the first signal succeeded, the second failed) the first
/// disposition is restored, the stop state reset, and the install marker
/// released, so no handler is left installed and a retry is possible.
pub fn install_termination_handler() -> Result<TerminationHandlerGuard, TerminationInstallError> {
    use std::sync::atomic::Ordering;

    // Single-active-install invariant: claim the marker before touching any
    // process-global state. A second install while a guard is alive is
    // rejected structurally, so restoration on drop always matches the one
    // active install.
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return Err(TerminationInstallError::AlreadyInstalled);
    }

    // Start from a clean stop state: a stale `true` from a previous install
    // must not leak into this one. The handler is not installed yet, so no
    // signal can race this reset.
    TERMINATION_REQUESTED.store(false, Ordering::SeqCst);

    // SAFETY: an all-zero `sigaction` is a valid starting point on Linux
    // (an all-zero `sigset_t` is the empty mask); every field the kernel
    // consumes is assigned before use.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = termination_handler as *const () as usize;
    // Deliberately no `SA_RESTART`: a pending signal must interrupt the
    // blocking read so the runtime can observe it (M5).
    action.sa_flags = 0;
    // SAFETY: `action.sa_mask` is a fully valid `sigset_t` and
    // `sigemptyset` zeroes it; `action` is fully initialized before use.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };

    // SAFETY: an all-zero `sigaction` is a valid out-parameter; the kernel
    // fills it completely.
    let mut previous_sigint: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: `action` is a fully initialized `sigaction`; `previous_sigint`
    // is a valid out-parameter the kernel fills.
    if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous_sigint) } < 0 {
        TERMINATION_REQUESTED.store(false, Ordering::SeqCst);
        INSTALLED.store(false, Ordering::SeqCst);
        return Err(TerminationInstallError::Install(io::Error::last_os_error()));
    }
    // SAFETY: as for `previous_sigint` above.
    let mut previous_sigterm: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: as for `SIGINT` above.
    if unsafe { libc::sigaction(libc::SIGTERM, &action, &mut previous_sigterm) } < 0 {
        // Partial failure: restore SIGINT and reset the stop state and the
        // install marker so no handler remains installed and a retry is
        // possible.
        // SAFETY: `previous_sigint` is the disposition in effect before our
        // install; `null_mut` out-parameter is valid.
        unsafe { libc::sigaction(libc::SIGINT, &previous_sigint, std::ptr::null_mut()) };
        TERMINATION_REQUESTED.store(false, Ordering::SeqCst);
        INSTALLED.store(false, Ordering::SeqCst);
        return Err(TerminationInstallError::Install(io::Error::last_os_error()));
    }
    Ok(TerminationHandlerGuard {
        previous_sigint,
        previous_sigterm,
        restored: false,
    })
}

/// Test helper: invokes the installed termination handler directly, as if a
/// signal had arrived. Used by the portable signal tests (no real signal is
/// delivered). Linux-only because the handler is only installed there.
#[cfg(all(target_os = "linux", test))]
pub(crate) fn fire_termination_handler_for_test() {
    termination_handler(libc::SIGINT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::SIGNAL_TEST_LOCK;

    /// M10 review R2: the poll revents classification is explicit and
    /// deterministic — `POLLIN`/`POLLHUP`/`POLLERR` are ready (the loop must
    /// read to surface the real data/EOF/error), `POLLNVAL` is an immediate
    /// structured error (never idle), and a pure timeout is idle. Every flag
    /// and combination is covered without a real fd.
    #[test]
    fn poll_revents_classification_is_explicit() {
        // POLLIN (data): ready.
        assert!(matches!(classify_revents(libc::POLLIN), Ok(true)));
        // POLLHUP (unplugged evdev fd): ready — the read surfaces EOF.
        assert!(matches!(classify_revents(libc::POLLHUP), Ok(true)));
        // POLLERR (failed fd): ready — the read surfaces the error.
        assert!(matches!(classify_revents(libc::POLLERR), Ok(true)));
        // POLLNVAL (invalid fd): an immediate structured error, never idle.
        assert!(matches!(
            classify_revents(libc::POLLNVAL),
            Err(SysError::InvalidArgument(_))
        ));
        // Timeout alone (no revents): idle.
        assert!(matches!(classify_revents(0), Ok(false)));

        // Combinations: HUP without IN, ERR without IN, and HUP|ERR must all
        // be ready (an unplugged/failed fd may wake without POLLIN).
        assert!(matches!(
            classify_revents(libc::POLLHUP | libc::POLLERR),
            Ok(true)
        ));
        assert!(matches!(
            classify_revents(libc::POLLIN | libc::POLLHUP),
            Ok(true)
        ));
        assert!(matches!(
            classify_revents(libc::POLLIN | libc::POLLERR | libc::POLLHUP),
            Ok(true)
        ));
        // NVAL wins over any other flag: the fd is invalid, no read can ever
        // make progress — an immediate structured error, never idle.
        assert!(matches!(
            classify_revents(libc::POLLNVAL | libc::POLLIN),
            Err(SysError::InvalidArgument(_))
        ));
        assert!(matches!(
            classify_revents(libc::POLLNVAL | libc::POLLHUP | libc::POLLERR),
            Err(SysError::InvalidArgument(_))
        ));
        // Unrelated revents (e.g. POLLPRI) with no progress bits: idle.
        assert!(matches!(classify_revents(libc::POLLPRI), Ok(false)));
    }

    /// The request encoders must agree with the well-known kernel constants;
    /// this re-checks them against the `libc`-independent canonical values
    /// used everywhere else.
    #[test]
    fn request_encoders_match_canonical_kernel_values() {
        assert_eq!(requests::eviocgrab(), 0x4004_4590);
        assert_eq!(requests::eviocgid(), 0x8008_4502);
        // _IOW('E', 0xa0, __u32)
        assert_eq!(requests::eviocsc_lockid(), 0x4004_45a0);
    }

    /// The byte layout the decoder assumes for `struct input_event` must
    /// match libc's definition on the supported live Linux target (x86_64,
    /// M4 reviews R3/RR3).
    #[test]
    fn input_event_abi_matches_libc() {
        assert_eq!(
            core::mem::size_of::<libc::input_event>(),
            crate::event::INPUT_EVENT_SIZE
        );
    }

    /// `read_dir` on a nonexistent path reports `NotFound` (no device is
    /// touched).
    #[test]
    fn read_dir_missing_path_is_not_found() {
        let sys = LinuxSys::new();
        let err = sys
            .read_dir(Path::new("/definitely/not/a/real/path/xyz"))
            .unwrap_err();
        assert!(matches!(err, SysError::NotFound { .. }), "got {err:?}");
    }

    /// Opening a nonexistent device node reports `NotFound` without panicking.
    #[test]
    fn open_missing_device_is_not_found() {
        let sys = LinuxSys::new();
        let err = sys.open(Path::new("/dev/input/event999999")).unwrap_err();
        assert!(
            matches!(
                err,
                SysError::NotFound { .. } | SysError::PermissionDenied { .. }
            ),
            "got {err:?}"
        );
    }

    /// M5: a **real** `SIGINT` delivered to the process records a stop
    /// request in the process-lifetime static (the end-to-end OS delivery
    /// path: signal → handler → stop state). The test installs the handler,
    /// raises SIGINT on the calling thread (the handler is installed, so the
    /// process is not terminated), asserts `termination_requested()`, and the
    /// guard restores the dispositions and resets the stop state.
    #[cfg(target_os = "linux")]
    #[test]
    fn real_sigint_records_the_stop_request() {
        let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
        let _guard = install_termination_handler().unwrap();
        assert!(!termination_requested());
        // SAFETY: `raise(2)` delivers SIGINT to the calling thread; our
        // handler is installed, so the default terminate action is replaced
        // and only the stop state is set.
        unsafe { libc::raise(libc::SIGINT) };
        assert!(termination_requested());
    }

    /// M5: dropping the guard restores the previous SIGINT/SIGTERM
    /// dispositions (verified by querying the current disposition with
    /// `sigaction`) and resets the stop state.
    #[cfg(target_os = "linux")]
    #[test]
    fn guard_drop_restores_the_previous_dispositions_and_resets_stop_state() {
        let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
        // Capture the disposition in effect at the start of the test.
        // SAFETY: an all-zero `sigaction` is a valid out-parameter; the
        // kernel fills it completely.
        let mut before: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: `before` is a valid out-parameter the kernel fills.
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGINT, std::ptr::null(), &mut before) },
            0
        );
        {
            let _guard = install_termination_handler().unwrap();
            // SAFETY: as for `before` above.
            let mut during: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: as above.
            assert_eq!(
                unsafe { libc::sigaction(libc::SIGINT, std::ptr::null(), &mut during) },
                0
            );
            assert_ne!(during.sa_sigaction, before.sa_sigaction);
            fire_termination_handler_for_test();
            assert!(termination_requested());
        }
        // SAFETY: as for `before` above.
        let mut after: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        assert_eq!(
            unsafe { libc::sigaction(libc::SIGINT, std::ptr::null(), &mut after) },
            0
        );
        assert_eq!(after.sa_sigaction, before.sa_sigaction);
        assert!(
            !termination_requested(),
            "guard teardown must reset the stop state"
        );
    }

    /// M5 re-review R1 model test: an in-flight handler that resumes after
    /// guard teardown touches only process-lifetime storage.
    ///
    /// The previously-unsafe interleaving was: a handler loads a pointer to
    /// caller-owned storage, is descheduled; another thread drops the guard
    /// (restores dispositions, clears the pointer, releases the last `Arc`);
    /// the handler resumes and dereferences freed memory. With the
    /// process-lifetime static design the handler dereferences **no**
    /// caller-owned memory, so the equivalent interleaving is deterministic
    /// and safe: fire the handler (its "target load" is a store to the
    /// static), drop the guard (teardown), then fire the handler again
    /// (modelling the in-flight invocation resuming after teardown). Both
    /// stores land in never-reclaimed `'static` storage — no undefined
    /// behavior is possible by construction.
    #[cfg(target_os = "linux")]
    #[test]
    fn in_flight_handler_resuming_after_teardown_touches_only_static_memory() {
        let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
        {
            let _guard = install_termination_handler().unwrap();
            // First invocation completes its store before teardown.
            fire_termination_handler_for_test();
            assert!(termination_requested());
        }
        // Teardown complete: dispositions restored, stop state reset. A
        // handler that "already loaded its target" and is now resuming
        // executes this exact store:
        fire_termination_handler_for_test();
        // It writes to the same process-lifetime static — nothing was freed,
        // the process is still sound, and the request is observable again.
        assert!(termination_requested());
        reset_termination_requested_for_test();
    }

    /// M5 review R1 (kept): a second install while the first guard is alive
    /// is rejected with a structured error, and the first handler keeps
    /// working.
    #[cfg(target_os = "linux")]
    #[test]
    fn second_install_is_rejected_with_structured_error() {
        let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
        let _guard_a = install_termination_handler().unwrap();
        let err = match install_termination_handler() {
            Err(error) => error,
            Ok(_) => panic!("a second install must be rejected"),
        };
        assert!(
            matches!(err, TerminationInstallError::AlreadyInstalled),
            "second install must be rejected, got {err:?}"
        );
        // The first guard's handler still works.
        fire_termination_handler_for_test();
        assert!(termination_requested());
    }

    /// M5 review R1 (kept): a fresh install succeeds after the first guard
    /// is dropped, starting from a clean stop state.
    #[cfg(target_os = "linux")]
    #[test]
    fn fresh_install_succeeds_after_the_first_guard_is_dropped() {
        let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
        {
            let _guard = install_termination_handler().unwrap();
            fire_termination_handler_for_test();
            assert!(termination_requested());
        }
        // The first guard is gone: a fresh install works and starts from a
        // clean stop state (the previous request does not leak).
        let _guard_b = install_termination_handler().unwrap();
        assert!(
            !termination_requested(),
            "a fresh install must start from a clean stop state"
        );
        fire_termination_handler_for_test();
        assert!(termination_requested());
    }
}
