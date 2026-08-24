//! The command environment: everything a command needs, so the whole CLI is
//! testable in-process with a mock [`Sys`] seam and in-memory writers.

use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use touchpad_core::Monotonic;
use touchpad_desktop::{DesktopOutputError, StreamingOutput};
use touchpad_linux::sys::{Fd, Sys, SysError};
use touchpad_linux::{RawEventRecorder, RecorderError};
use touchpad_trace::TraceHeader;

/// Builds the record command's raw-event recorder (M5 review R2/R3 fault
/// injection). The real binary uses [`touchpad_linux::TraceRecorder::create`];
/// tests inject recorders that fail on flush/finish or record timeline
/// markers, so the command-level ordering and failure semantics are provable
/// in-process.
pub type RecorderFactory =
    Box<dyn Fn(&Path, &TraceHeader) -> Result<Box<dyn RawEventRecorder>, RecorderError>>;

/// Builds the desktop output backend for `output-probe` (M6). The real
/// binary uses the portal/libei backend; tests inject
/// [`touchpad_desktop::FakeDesktopOutput`], so no test ever touches the
/// real portal, libei, or the desktop.
pub type OutputFactory = Box<dyn FnMut() -> Box<dyn touchpad_desktop::DesktopOutput>>;

/// An injectable monotonic clock (M10): the takeover loop checks its maximum
/// duration deadline against this clock, so tests can advance a fake clock
/// deterministically without sleeping. The real binary uses a wall-free
/// monotonic clock ([`std::time::Instant`]-based).
pub type ClockFn = Rc<dyn Fn() -> Monotonic>;

/// An injectable bounded-readiness seam (M10): returns whether a read on
/// `fd` would not block, waiting at most `timeout`. The takeover loop wakes
/// at a short fixed quantum, checks the clock/stop/fault, and reads only
/// when this returns `true`. Tests inject a fake that advances a fake clock
/// by the timeout and returns scripted readiness; the real binary polls the
/// fd with `poll(2)` (implemented in `touchpad-linux`'s existing unsafe FFI
/// boundary).
pub type ReadinessFn = Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>>;

/// An injectable sleeper (M10): the takeover countdown and per-tick waits
/// use it, so tests can run the countdown with a no-op (never sleeping) and
/// the real binary sleeps.
pub type SleeperFn = Rc<dyn Fn(Duration)>;

/// Builds the M10 streaming output session (M10_TASK.md §5). The real binary
/// uses [`touchpad_desktop::RealStreamingOutputFactory`]; tests inject
/// [`touchpad_desktop::FakeStreamingOutputFactory`], so no test ever
/// constructs the real portal, libei, or desktop session.
pub type StreamingOutputFactoryFn =
    Box<dyn FnMut() -> Result<Box<dyn StreamingOutput>, DesktopOutputError>>;

/// The M10 takeover seams (clock, readiness, sleeper, streaming factory).
///
/// These are only used by `touchpadctl takeover`; every other command uses
/// [`TakeoverSeams::inert`] defaults.
pub struct TakeoverSeams {
    /// The monotonic clock the takeover deadline is measured against.
    pub clock: ClockFn,
    /// The bounded-readiness seam (poll-with-timeout) of the event loop.
    pub readiness: ReadinessFn,
    /// The sleeper used by the pre-takeover countdown.
    pub sleeper: SleeperFn,
    /// The streaming output session factory (`None` = the real backend).
    pub streaming_factory: Option<StreamingOutputFactoryFn>,
}

impl TakeoverSeams {
    /// Inert defaults for commands that do not run the takeover loop: a
    /// zero clock, a never-ready readiness, a no-op sleeper, and no
    /// streaming factory. Any use of these by a non-takeover command is a
    /// test-harness error, not a runtime path.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            clock: Rc::new(|| Monotonic::ZERO),
            readiness: Rc::new(|_, _| Ok(false)),
            sleeper: Rc::new(|_| {}),
            streaming_factory: None,
        }
    }
}

/// Everything a command needs to run.
///
/// `sys` is the mockable OS seam ([`touchpad_linux::sys::MockSys`] in tests,
/// [`touchpad_linux::sys::ffi::LinuxSys`] for the real binary), `out`/`err`
/// are the standard output / diagnostics writers, and `stop_flag` is an
/// **injectable stop source**: the real `SIGINT`/`SIGTERM` handler records a
/// stop request in a process-lifetime static consulted via
/// [`touchpad_linux::termination_requested`] (M5 re-review R1 — the handler
/// dereferences no caller-owned memory), while tests set `stop_flag` to
/// simulate a stop deterministically. The record command polls both between
/// blocking operations; the takeover command polls them in its bounded loop.
pub struct CommandEnv<'a> {
    /// The OS seam (device enumeration/open/ioctl/read).
    pub sys: Rc<dyn Sys>,
    /// Standard output (machine-readable command output).
    pub out: &'a mut dyn Write,
    /// Diagnostics / summaries / status.
    pub err: &'a mut dyn Write,
    /// Injectable stop source (tests simulate a signal by setting it); the
    /// real signal handler uses the process-lifetime static observed via
    /// [`touchpad_linux::termination_requested`].
    pub stop_flag: Arc<AtomicBool>,
    /// Optional recorder factory (fault injection / shared-timeline tests).
    /// `None` means the default [`touchpad_linux::TraceRecorder::create`].
    pub recorder_factory: Option<RecorderFactory>,
    /// Optional desktop output factory for `output-probe` (M6). `None`
    /// means the default portal/libei backend (`PortalDesktopOutput` on
    /// Linux, `UnsupportedDesktopOutput` elsewhere); tests inject
    /// [`touchpad_desktop::FakeDesktopOutput`].
    pub output_factory: Option<OutputFactory>,
    /// The M10 takeover seams (clock, readiness, sleeper, streaming
    /// factory). Only `takeover` uses them.
    pub takeover: TakeoverSeams,
}
