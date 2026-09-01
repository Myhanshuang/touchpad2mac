//! The evdev input runtime: open → read/decode loop → controlled shutdown
//! (M4 requirement 6), with the M5 raw-event recorder and signal stop.
//!
//! [`EvdevRuntime`] owns the open device (through the [`crate::grab`]
//! RAII guard), the Type-B decoder (with the real [`crate::snapshot`]
//! resync adapter installed), and the explicit shutdown lifecycle:
//!
//! ```text
//! 1. stop work        — the phase leaves `Running`; `step()` is refused
//! 2. end output/finish— M5: the recorder (if attached) is completely
//!                       finalized here — `finish` (which flushes) plus
//!                       best-effort recorder destruction, so buffered bytes
//!                       reach the sink before the release; the
//!                       semantic-output lifecycle is an explicitly
//!                       documented no-op (Phase 1 has no real output
//!                       backend)
//! 3. idempotent ungrab— `EVIOCGRAB(0)` at most once (even on failure), safe
//!                       on repeated calls
//! 4. close fd         — idempotent, after the ungrab
//! ```
//!
//! ## Raw-event recorder (M5)
//!
//! A [`RawEventRecorder`] attached with [`EvdevRuntime::set_recorder`] sits
//! **in front of the decoder**: every kernel event decoded from a read batch
//! is recorded before it is fed to the decoder (IMPLEMENTATION_BRIEF §8), so
//! a decoder bug can never lose the raw input needed to reproduce it, and a
//! recorder failure is fatal for the session but never erases already
//! recorded events. The recorder must be attached before the first step. On
//! every shutdown path (normal, signal, fatal stream/decoder error) the
//! recorder's complete fallible finalization — `finish`, plus the best-
//! effort destruction needed to flush buffered bytes when `finish` fails —
//! happens **before** the grab is released (step 2 above, M5 re-review R3);
//! the fallible `finish` is never called after the device release.
//!
//! ## Signal stop (M5)
//!
//! A stop flag (an [`std::sync::atomic::AtomicBool`]) can be attached with
//! [`EvdevRuntime::set_stop_flag`]; the CLI's `SIGINT`/`SIGTERM` handler
//! sets it ([`crate::signals`]). When the blocking `read` is interrupted by
//! a signal (`EINTR`, surfaced as [`crate::sys::SysError::Interrupted`]) and
//! the stop flag is set, [`EvdevRuntime::step`] returns
//! [`RuntimeError::Interrupted`] — a graceful stop request — after
//! transitioning to [`RuntimePhase::Stopping`] **without** fail-opening the
//! device, so the caller can run the ordered shutdown (recorder
//! finalization → ungrab → close). An `EINTR` with no stop requested keeps
//! the M4
//! behavior: it is an ordinary fatal error and the runtime fails open. The
//! caller additionally polls the flag between steps to cover signals that
//! arrive while the runtime is not blocked in a read.
//!
//! ## M10 takeover additions (bounded loop support)
//!
//! [`EvdevRuntime::step_deferred`] is the M10 takeover loop's step path: on a
//! fatal stream/decoder/recorder error it stops accepting new work
//! ([`RuntimePhase::Stopping`]) but **defers** the immediate fail-open
//! cleanup, leaving the output session, recorder, grab, and fd available for
//! the M10 coordinator's unified ordered shutdown (output release → recorder
//! finalize → ungrab → close). [`EvdevRuntime::sink_mut`]/
//! [`EvdevRuntime::take_sink`] expose the decoder's frame sink (the M10
//! bridge) so the coordinator can prepare/release the virtual output session
//! around the loop, and [`EvdevRuntime::fd`] exposes the session fd for the
//! bounded-readiness poll. [`EvdevRuntime::step`] keeps its M4/M5 semantics
//! unchanged.
//!
//! ## Open contract (M4 review R1/R4; grab timing M5 review R2)
//!
//! [`EvdevRuntime::open`] opens the device **once** for the session and
//! performs every capability/axis/slot validation on that exact fd (shared
//! [`crate::device::probe_open_fd`] logic with enumeration), so a path swap
//! or device removal between a probe fd and the session fd cannot attach one
//! device's validation to another device. It then selects the monotonic
//! clock on that fd (`EVIOCSCLOCKID(CLOCK_MONOTONIC)` — evdev defaults to
//! `INPUT_CLK_REAL`, so monotonic timestamps are *not* by construction) and
//! prepares the decoder and snapshot adapter. **`open` never grabs** (M5
//! review R2): the optional exclusive grab is issued by the caller through
//! the checked [`EvdevRuntime::grab`] method — the record command calls it
//! only after the recorder was created and its header successfully flushed,
//! immediately before the read loop. Any preparation failure closes the fd
//! and returns an actionable [`OpenError`]; the grab is never reached on an
//! open failure path.
//!
//! ## Resync drain rule (M4 review R6)
//!
//! A single `read` batch may contain `SYN_DROPPED`, the recovery
//! `SYN_REPORT`, **and** later events. The snapshot ioctl observes kernel
//! state that already includes those later events, so replaying the rest of
//! the batch would apply pre-snapshot deltas (tracking-id lifecycles, button
//! changes) on top of the newer snapshot and emit false transitions.
//! [`EvdevRuntime::step`] therefore stops feeding the batch as soon as the
//! decoder reports a successful resync ([`TypeBDecoder::just_resynced`]);
//! the remainder of the batch is drained (they are part of the dropped
//! window and inherently lost), and the next read begins from state
//! consistent with the snapshot. This is the documented fail-closed
//! synchronization boundary: evdev's queue can hold events that predate the
//! snapshot ioctl, so discarding them is the only way to never emit a stale
//! lifecycle or frame after a discontinuity. **The recorder still records
//! every event of the batch** (it captures what the kernel delivered, before
//! any decoding), so the trace remains the ground truth of the raw stream;
//! only the decoder's *output* suppresses the stale deltas.
//!
//! Fail-open behavior (M4 requirement 4): any fatal stream/decoder/recorder
//! error — device unplugged (EOF), torn read, `EINTR` without a stop
//! request, timestamp regression, invalid event payload, or a decoder
//! failure (including a failed `SYN_DROPPED` resynchronization) — stops
//! frame production, completes the recorder's fallible finalization
//! (`finish` plus best-effort destruction, before the release, M5 re-review
//! R3), releases the grab (at most once, best-effort), closes the fd, and
//! returns an actionable [`RuntimeError`]. Nothing panics.
//!
//! ## Ordered fallback destruction (M5 review R4, extended by R3)
//!
//! [`Drop`] is an ordered best-effort fallback for paths that skip the
//! explicit shutdown — early `?` returns after the recorder was attached
//! (e.g. a failing status writer in the record command) and unexpected
//! unwinds: it completes the recorder's fallible finalization (`finish`,
//! best-effort) **and destroys the recorder** before the `DeviceHandle`
//! field is dropped, so both the `finish` and the recorder's best-effort
//! `Drop` flush always precede the device release (ungrab, then close, each
//! at most once) even on fallback destruction. Explicit
//! `shutdown()`/`fail_open` remain the primary paths and mark the runtime
//! [`RuntimePhase::Stopped`]; `Drop` then has nothing left to do.
//!
//! ## Threading and signals
//!
//! M4/M5 are single-threaded: [`EvdevRuntime::step`] blocks on the
//! underlying read. Real signal handling (mapping `SIGINT`/`SIGTERM` +
//! `EINTR` to a graceful shutdown) is implemented by [`crate::signals`] and
//! this runtime's stop-flag handling (see above); the CLI belongs to M5.
//!
//! ## Cleanup guarantees
//!
//! `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
//! cleanup: the kernel releases an evdev grab automatically when the owning
//! fd is closed by the process exit, but no userspace `ungrab`/`close`
//! ordering can be guaranteed in those cases. No real-hardware behavior is
//! claimed by this milestone; everything here is exercised through the
//! mockable [`crate::sys`] seam.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use touchpad_core::{DeviceDescriptor, Monotonic};

use crate::codes::{
    ABS_MT_ORIENTATION, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE, ABS_MT_TOUCH_MAJOR,
    ABS_MT_TOUCH_MINOR,
};
use crate::decode::{DecodeError, TypeBDecoder};
use crate::device;
use crate::event::{self, EventDecodeError, KernelEvent, TimevalError};
use crate::grab::{DeviceHandle, GrabError};
use crate::recorder::{RawEventRecorder, RecorderError};
use crate::sink::FrameSink;
use crate::snapshot::EvdevSnapshotSource;
use crate::sys::{Fd, InputId, Sys, SysError};

/// Number of kernel events read per [`EvdevRuntime::step`].
pub const READ_BUFFER_EVENTS: usize = 64;
/// Size of the read buffer in bytes (whole `input_event` structs).
pub const READ_BUFFER_BYTES: usize = READ_BUFFER_EVENTS * event::INPUT_EVENT_SIZE;

/// Runtime lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePhase {
    /// Accepting new work ([`EvdevRuntime::step`] may run).
    Running,
    /// Shutdown started; no new work is accepted.
    Stopping,
    /// Shutdown complete (or a fatal error closed the device); the runtime
    /// is inert.
    Stopped,
}

/// Failure of [`EvdevRuntime::open`].
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The device exists but does not qualify as a touchpad candidate; the
    /// reasons are the explainable rejection from the probe.
    #[error("device at {path} does not qualify as a touchpad candidate: {}", reasons.join("; "))]
    NotCandidate {
        /// The device node.
        path: PathBuf,
        /// The probe's rejection reasons.
        reasons: Vec<String>,
    },
    /// The device could not be probed (permission, ioctl failure, ...).
    #[error("device at {path} could not be probed: {message}")]
    Probe {
        /// The device node.
        path: PathBuf,
        /// The probe failure message.
        message: String,
    },
    /// The session open of the device node failed.
    #[error("could not open device at {path}: {source}")]
    Access {
        /// The device node.
        path: PathBuf,
        /// Why the open failed.
        source: SysError,
    },
    /// The decoder rejected the probed descriptor.
    #[error("could not configure the decoder for device at {path}: {source}")]
    Configure {
        /// The device node.
        path: PathBuf,
        /// The decoder's rejection.
        source: DecodeError,
    },
    /// The real resync snapshot adapter could not be built (e.g. an
    /// out-of-range slot count).
    #[error("could not build the resync snapshot adapter: {message}")]
    SnapshotSource {
        /// Why the adapter could not be built.
        message: String,
    },
    /// The monotonic clock could not be selected on the session fd via
    /// `EVIOCSCLOCKID`. evdev defaults to `INPUT_CLK_REAL`, so without this
    /// the stream would be mislabeled monotonic; the failure is actionable,
    /// the fd is closed, and the runtime never grabs or starts (M4 review
    /// R1).
    #[error("could not select CLOCK_MONOTONIC on device at {path}: {source}")]
    Clock {
        /// The device node.
        path: PathBuf,
        /// Why the ioctl failed.
        source: SysError,
    },
}

/// Failure of a runtime operation.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The device could not be opened/probed/configured.
    #[error(transparent)]
    Open(#[from] OpenError),
    /// A step was attempted with no open device.
    #[error("runtime has no open device")]
    NotOpen,
    /// A step was attempted after shutdown or a fatal error.
    #[error("runtime is not running (shutdown or a fatal error already occurred)")]
    NotRunning,
    /// The underlying read failed (including `EINTR`).
    #[error("read failed: {0}")]
    Read(SysError),
    /// The device was unplugged (EOF from the event node).
    #[error("device disconnected (end of stream from the event node)")]
    DeviceGone,
    /// A read returned a byte count that is not a multiple of the event
    /// size; the kernel never produces torn events.
    #[error("read returned {actual} bytes, not a multiple of the {event_size}-byte input_event size; torn event data")]
    PartialRead {
        /// Bytes actually read.
        actual: usize,
        /// The per-event size.
        event_size: usize,
    },
    /// The kernel monotonic clock went backwards.
    #[error("event timestamp regression: previous {previous:?}, current {current:?}")]
    TimestampRegression {
        /// The previous timestamp.
        previous: Monotonic,
        /// The regressing timestamp.
        current: Monotonic,
    },
    /// A kernel event payload could not be decoded.
    #[error("invalid event payload: {0}")]
    Event(EventDecodeError),
    /// A kernel event's monotonic timeval is invalid (negative, out of
    /// range, or overflowing).
    #[error("invalid event timeval: {0}")]
    Timeval(TimevalError),
    /// The decoder failed (including a fatal `SYN_DROPPED` resync failure,
    /// after which it is degraded and emits no trusted frames).
    #[error("decoder failure: {0}")]
    Decode(DecodeError),
    /// The raw-event recorder failed (I/O, poisoned trace writer, or an
    /// unrepresentable event timeval). Fatal for the session; events already
    /// recorded stay recorded.
    #[error("recorder failure: {0}")]
    Recorder(RecorderError),
    /// A grab/ungrab operation failed.
    #[error("grab operation failed: {0}")]
    Grab(GrabError),
    /// A grab was requested after the first step already read events; the
    /// grab must precede the read loop (M5 review R2 — the record command
    /// grabs only after the recorder is prepared and before the first read).
    #[error("cannot grab after the first step; EVIOCGRAB(1) must precede the read loop")]
    GrabAfterStep,
    /// The blocking read was interrupted by a signal **after a stop was
    /// requested** (the stop flag is set, M5): the runtime stopped accepting
    /// new work ([`RuntimePhase::Stopping`]) and deliberately left the device
    /// open so the caller can run the ordered shutdown (recorder
    /// finalization → ungrab → close). This is a graceful stop request,
    /// **not** an ordinary fatal stream error: the caller must call
    /// [`EvdevRuntime::shutdown`].
    #[error(
        "read interrupted by a signal after a stop was requested; the runtime stopped accepting new work and the device is left open for the ordered shutdown"
    )]
    Interrupted,
}

/// Result of the controlled shutdown (and of the fatal fail-open cleanup).
///
/// One structured report carries the results of the *whole* ordered
/// finalization — recorder finalization, ungrab, close — so no cleanup
/// failure is ever lost or misattributed (M5 re-review R3).
#[derive(Debug)]
pub struct ShutdownReport {
    /// The phase after finalization (always [`RuntimePhase::Stopped`]).
    pub phase: RuntimePhase,
    /// The recorder's complete fallible finalization result
    /// ([`RawEventRecorder::finish`], which flushes; step 2 of the ordered
    /// finalization, before the ungrab); `None` when no recorder is
    /// attached. The recorder is **destroyed as part of finalization** (its
    /// best-effort `Drop` flush is the last chance to push buffered bytes
    /// when `finish` failed), so a later `finish` after the device release
    /// is impossible by construction.
    pub recorder_finish: Option<Result<(), RecorderError>>,
    /// Raw events recorded before finalization (for truthful status
    /// reporting); `0` when no recorder was attached. Captured before the
    /// recorder is destroyed.
    pub events_recorded: u64,
    /// The idempotent ungrab result; `None` when no device was held (e.g.
    /// repeated shutdown, or shutdown after a fail-open already released
    /// it).
    pub ungrab: Option<Result<(), GrabError>>,
    /// The idempotent fd-close result; `None` when no device was held.
    pub close: Option<Result<(), SysError>>,
}

/// The evdev input runtime.
///
/// Owns the device handle (optionally grabbed), the Type-B decoder with the
/// real snapshot resync adapter, the optional raw-event recorder (in front
/// of the decoder, M5), and the controlled shutdown lifecycle described at
/// the module top.
pub struct EvdevRuntime<S: FrameSink> {
    sys: Rc<dyn Sys>,
    device: Option<DeviceHandle>,
    /// Kernel identity captured from the exact session fd during the shared
    /// capability probe. DWT keyboard pairing reuses this value instead of
    /// issuing a second identity ioctl later.
    input_id: InputId,
    /// The Type-B decoder, always present until [`EvdevRuntime::into_sink`]
    /// takes it out (an `Option` so the consuming accessor can move it out
    /// despite the ordered `Drop` impl, M5 review R4).
    decoder: Option<TypeBDecoder<S>>,
    phase: RuntimePhase,
    buf: Vec<u8>,
    last_timestamp: Option<Monotonic>,
    /// The raw-event recorder attached with [`EvdevRuntime::set_recorder`]
    /// (before the decoder, M5). `None` when recording is disabled.
    recorder: Option<Box<dyn RawEventRecorder>>,
    /// The stop flag attached with [`EvdevRuntime::set_stop_flag`] (M5);
    /// when set, an interrupted read becomes a graceful stop.
    stop_flag: Option<Arc<AtomicBool>>,
    /// The validated device descriptor from [`EvdevRuntime::open`] (exposed
    /// for recorder header construction and inspection).
    descriptor: Option<DeviceDescriptor>,
    /// Whether [`EvdevRuntime::step`] ever ran. [`EvdevRuntime::grab`] is
    /// rejected afterwards: the grab must precede the read loop (M5 review
    /// R2).
    stepped: bool,
    /// The cleanup report of the most recent fail-open (fatal error path,
    /// M5 review R3). Lets the caller observe recorder-finalization / ungrab
    /// / close failures that the fail-open performed instead of discarding
    /// them; taken by [`EvdevRuntime::take_fail_open_report`].
    fail_open_report: Option<ShutdownReport>,
}

impl<S: FrameSink> EvdevRuntime<S> {
    /// Opens `path` for a runtime session (M4 review R4).
    ///
    /// The device is opened **once**; every capability/axis/slot validation
    /// runs on that exact fd (the same [`crate::device::probe_open_fd`]
    /// rules as enumeration, so they cannot drift). After validation the
    /// monotonic clock is selected on that fd (`EVIOCSCLOCKID`), the decoder
    /// is configured with the probed descriptor, and the real `SYN_DROPPED`
    /// snapshot adapter is installed. Any preparation failure closes the fd
    /// and returns an actionable [`OpenError`].
    ///
    /// **`open` never grabs** (M5 review R2): the optional exclusive grab is
    /// issued later by the caller through the checked
    /// [`EvdevRuntime::grab`], after the recorder/output is prepared and
    /// immediately before the first read.
    pub fn open(sys: Rc<dyn Sys>, path: &Path, sink: S) -> Result<Self, RuntimeError> {
        // Open exactly once for this session.
        let mut handle =
            DeviceHandle::open(Rc::clone(&sys), path).map_err(|source| OpenError::Access {
                path: path.to_path_buf(),
                source,
            })?;
        let fd = handle
            .fd()
            .expect("a freshly opened device handle always has an fd");

        // Validate capabilities/axes/slot on the session fd.
        let data = device::probe_open_fd(&*sys, fd).map_err(|source| {
            let _ = handle.close();
            OpenError::Probe {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        let mut evidence = Vec::new();
        let descriptor = match device::decide_verdict(
            &data.name,
            data.id,
            &data.capabilities,
            &data.axes,
            data.slot_count,
            &mut evidence,
        ) {
            Ok(descriptor) => descriptor,
            Err(reasons) => {
                let _ = handle.close();
                return Err(OpenError::NotCandidate {
                    path: path.to_path_buf(),
                    reasons,
                }
                .into());
            }
        };

        // Select CLOCK_MONOTONIC on this exact fd before grab and before any
        // read: the evdev client clock defaults to INPUT_CLK_REAL (0) and
        // only switches after EVIOCSCLOCKID (M4 review R1). A failure is an
        // actionable setup error: close the fd, never grab or start.
        sys.ioctl_set_clock_id(fd, crate::sys::CLOCK_MONOTONIC)
            .map_err(|source| {
                let _ = handle.close();
                OpenError::Clock {
                    path: path.to_path_buf(),
                    source,
                }
            })?;

        // Prepare the decoder and the real resync snapshot adapter.
        let mut decoder = TypeBDecoder::new(sink);
        decoder.configure(descriptor.clone()).map_err(|source| {
            let _ = handle.close();
            OpenError::Configure {
                path: path.to_path_buf(),
                source,
            }
        })?;
        let slot_count = descriptor.slot_count.ok_or_else(|| OpenError::Configure {
            path: path.to_path_buf(),
            source: DecodeError::InvalidDevice(
                "candidate descriptor has no Type-B slot count".to_string(),
            ),
        })?;
        // The Linux layer maps ABS codes onto AxisIds one-to-one, so the
        // descriptor's axis keys are the ABS codes of the MT axes the device
        // reports.
        let mt_axes: Vec<u16> = descriptor
            .axes
            .keys()
            .map(|axis| axis.as_u32() as u16)
            .filter(|code| {
                matches!(
                    code,
                    &ABS_MT_POSITION_X
                        | &ABS_MT_POSITION_Y
                        | &ABS_MT_PRESSURE
                        | &ABS_MT_TOUCH_MAJOR
                        | &ABS_MT_TOUCH_MINOR
                        | &ABS_MT_ORIENTATION
                )
            })
            .collect();
        let snapshot = EvdevSnapshotSource::new(
            Rc::clone(&sys),
            fd,
            slot_count,
            mt_axes,
            descriptor.has_physical_buttons,
        )
        .map_err(|error| {
            let _ = handle.close();
            OpenError::SnapshotSource {
                message: error.to_string(),
            }
        })?;
        decoder.set_resync_source(Box::new(snapshot));

        // No grab here (M5 review R2): the caller issues the optional
        // exclusive grab through `EvdevRuntime::grab` after the recorder is
        // prepared, immediately before the read loop.

        Ok(Self {
            sys,
            device: Some(handle),
            input_id: data.id,
            decoder: Some(decoder),
            phase: RuntimePhase::Running,
            buf: vec![0u8; READ_BUFFER_BYTES],
            last_timestamp: None,
            recorder: None,
            stop_flag: None,
            descriptor: Some(descriptor),
            stepped: false,
            fail_open_report: None,
        })
    }

    /// Explicitly grabs the device (`EVIOCGRAB(1)`) on the session fd (M5
    /// review R2).
    ///
    /// This is the runtime's checked, state-correct grab interface: it
    /// requires the runtime to be [`RuntimePhase::Running`] with the device
    /// open, and it is idempotent (grabbing an already-grabbed device is a
    /// no-op, so exactly one `EVIOCGRAB(1)` is issued). It is rejected after
    /// the first [`EvdevRuntime::step`] — the grab must precede the read
    /// loop — and after shutdown.
    ///
    /// The record command calls this only after the recorder was created
    /// from the same validated descriptor and its header was successfully
    /// flushed, immediately before the read loop: grab is the *last*
    /// preparation step, so an unwritable output or a failed header flush
    /// issues zero grab calls.
    pub fn grab(&mut self) -> Result<(), RuntimeError> {
        if self.phase != RuntimePhase::Running {
            return Err(RuntimeError::NotRunning);
        }
        if self.stepped {
            return Err(RuntimeError::GrabAfterStep);
        }
        let device = self.device.as_mut().ok_or(RuntimeError::NotOpen)?;
        device.grab().map_err(RuntimeError::Grab)
    }

    /// Whether `EVIOCGRAB(1)` is currently held on the session fd.
    #[must_use]
    pub fn is_grabbed(&self) -> bool {
        self.device.as_ref().is_some_and(DeviceHandle::is_grabbed)
    }

    /// Attaches the raw-event recorder (M5).
    ///
    /// The recorder sits **in front of the decoder**: from the next
    /// [`EvdevRuntime::step`] on, every kernel event decoded from a read
    /// batch is recorded before it is fed to the decoder, so a decoder bug
    /// cannot lose the raw input needed to reproduce it. Must be called
    /// before the first step (events of earlier steps would be missing from
    /// the trace). Replacing a recorder drops the previous one (its `Drop`
    /// flushes best-effort); recorders are expected to be attached once.
    pub fn set_recorder(&mut self, recorder: Box<dyn RawEventRecorder>) {
        self.recorder = Some(recorder);
    }

    /// Attaches an injectable stop flag (M5).
    ///
    /// When the blocking read is interrupted by a signal and a stop was
    /// requested — this attached flag set **or** the process-lifetime
    /// termination static set ([`crate::signals::termination_requested`],
    /// M5 re-review R1) — [`EvdevRuntime::step`] returns
    /// [`RuntimeError::Interrupted`] (graceful stop; the device is left open
    /// for the ordered shutdown) instead of treating the `EINTR` as an
    /// ordinary fatal error. The attached flag is an injectable stop source
    /// (tests simulate a signal with it); real `SIGINT`/`SIGTERM` deliveries
    /// set the static. Callers should poll the stop state between steps as
    /// well, to cover signals that arrive while the runtime is not blocked
    /// in a read.
    pub fn set_stop_flag(&mut self, flag: Arc<AtomicBool>) {
        self.stop_flag = Some(flag);
    }

    /// The validated device descriptor from [`EvdevRuntime::open`], if any.
    #[must_use]
    pub fn descriptor(&self) -> Option<&DeviceDescriptor> {
        self.descriptor.as_ref()
    }

    /// Kernel `input_id` captured from the exact session fd during open.
    #[must_use]
    pub const fn input_id(&self) -> InputId {
        self.input_id
    }

    /// The attached recorder, if any (for status reporting such as
    /// [`RawEventRecorder::events_recorded`]).
    #[must_use]
    pub fn recorder(&self) -> Option<&dyn RawEventRecorder> {
        self.recorder
            .as_deref()
            .map(|recorder| recorder as &dyn RawEventRecorder)
    }

    /// Takes the attached recorder out of the runtime. Returns `None` when
    /// no recorder is attached **or** after finalization: the ordered
    /// finalization ([`EvdevRuntime::shutdown`], [`EvdevRuntime::fail_open`],
    /// and the fallback `Drop`) destroys the recorder as part of completing
    /// its fallible finalization (M5 re-review R3), so the fallible
    /// [`RawEventRecorder::finish`] is called by the runtime — never by a
    /// caller after the device was released.
    pub fn into_recorder(&mut self) -> Option<Box<dyn RawEventRecorder>> {
        self.recorder.take()
    }

    /// Reads one batch of kernel events from the device and feeds them to
    /// the decoder, returning the number of raw events fed. Frames are
    /// published to the sink at `SYN_REPORT` boundaries.
    ///
    /// The raw-event recorder (M5) is invoked for every decoded event
    /// **before** the decoder sees it, so a decoder failure cannot lose the
    /// raw input. A recorder failure is fatal for the session (fail-open,
    /// with the recorder finalized before the grab release).
    ///
    /// A blocking read interrupted by a signal returns
    /// [`RuntimeError::Interrupted`] (graceful stop, device left open) when
    /// the stop flag is set; otherwise `EINTR` keeps its M4 semantics (an
    /// ordinary fatal error, fail-open).
    ///
    /// On any fatal stream/decoder/recorder error this performs the
    /// fail-open cleanup — finalize the recorder (finish + best-effort
    /// destruction), release the grab (if held), close the fd, mark the
    /// runtime [`RuntimePhase::Stopped`] — and returns the actionable
    /// error.
    pub fn step(&mut self) -> Result<usize, RuntimeError> {
        self.step_impl(false)
    }

    /// Same read/record/decode semantics as [`EvdevRuntime::step`], but with
    /// **deferred cleanup** for the M10 takeover loop (M10_TASK.md §7): on a
    /// fatal stream/decoder/recorder error the runtime stops accepting new
    /// work ([`RuntimePhase::Stopping`]) and returns the error **without**
    /// running the immediate fail-open cleanup — the output session, the
    /// recorder, the grab, and the fd stay available so the caller (the M10
    /// coordinator) can run the **unified ordered shutdown** (virtual output
    /// release → recorder finalization → ungrab → close) and preserve every
    /// cleanup result. [`EvdevRuntime::shutdown`] performs that sequence;
    /// the caller must call it after a deferred error instead of dropping
    /// the runtime. The `EINTR`-with-stop path is unchanged
    /// ([`RuntimeError::Interrupted`], graceful, device left open).
    pub fn step_deferred(&mut self) -> Result<usize, RuntimeError> {
        self.step_impl(true)
    }

    fn step_impl(&mut self, defer_cleanup: bool) -> Result<usize, RuntimeError> {
        if self.phase != RuntimePhase::Running {
            return Err(RuntimeError::NotRunning);
        }
        let fd = self
            .device
            .as_ref()
            .and_then(DeviceHandle::fd)
            .ok_or(RuntimeError::NotOpen)?;
        // M5 review R2: from the first step on, a later `grab()` is rejected
        // (the grab must precede the read loop).
        self.stepped = true;
        let n = match self.sys.read(fd, &mut self.buf) {
            Err(SysError::Interrupted) if self.stop_requested() => {
                // M5: our SIGINT/SIGTERM woke the blocking read. Stop
                // accepting new work but leave the device open so the caller
                // can run the ordered shutdown (recorder finalization →
                // ungrab → close); this is a graceful stop, not an ordinary
                // fatal error.
                self.phase = RuntimePhase::Stopping;
                return Err(RuntimeError::Interrupted);
            }
            Err(error) => {
                return Err(self.defer_or_fail_open(RuntimeError::Read(error), defer_cleanup));
            }
            Ok(n) => n,
        };
        if n == 0 {
            return Err(self.defer_or_fail_open(RuntimeError::DeviceGone, defer_cleanup));
        }
        if n % event::INPUT_EVENT_SIZE != 0 {
            return Err(self.defer_or_fail_open(
                RuntimeError::PartialRead {
                    actual: n,
                    event_size: event::INPUT_EVENT_SIZE,
                },
                defer_cleanup,
            ));
        }
        let events = event::decode_input_events(&self.buf[..n])
            .map_err(|error| self.defer_or_fail_open(RuntimeError::Event(error), defer_cleanup))?;

        // M5: record every raw event read from the device BEFORE the decoder
        // sees it (IMPLEMENTATION_BRIEF §8). A recorder failure is fatal for
        // the session, but events already recorded stay recorded.
        if let Err(error) = self.record_all(&events) {
            return Err(self.defer_or_fail_open(RuntimeError::Recorder(error), defer_cleanup));
        }

        let mut fed = 0;
        for kernel_event in events {
            let raw = kernel_event.to_raw_event().map_err(|error| {
                self.defer_or_fail_open(RuntimeError::Timeval(error), defer_cleanup)
            })?;
            // The kernel monotonic clock never regresses; a regression means
            // the driver/stream is untrustworthy, so the runtime fails open.
            if let Some(previous) = self.last_timestamp {
                if raw.timestamp < previous {
                    return Err(self.defer_or_fail_open(
                        RuntimeError::TimestampRegression {
                            previous,
                            current: raw.timestamp,
                        },
                        defer_cleanup,
                    ));
                }
            }
            self.last_timestamp = Some(raw.timestamp);
            self.decoder
                .as_mut()
                .expect("the decoder is always present until into_sink")
                .feed(raw)
                .map_err(|error| {
                    self.defer_or_fail_open(RuntimeError::Decode(error), defer_cleanup)
                })?;
            fed += 1;
            if self
                .decoder
                .as_ref()
                .expect("the decoder is always present until into_sink")
                .just_resynced()
            {
                // M4 review R6: the resync snapshot observed kernel state
                // that already includes the events queued after this batch's
                // recovery SYN_REPORT; replaying the rest of the batch would
                // apply pre-snapshot deltas (tracking-id lifecycles, button
                // changes) on top of the newer snapshot. Drain the remainder
                // of the batch — those events are part of the dropped window
                // and inherently lost — so no stale lifecycle or frame is
                // emitted after the discontinuity frame. (The recorder still
                // recorded the whole batch above: the trace is the ground
                // truth of the raw stream.)
                break;
            }
        }
        Ok(fed)
    }

    /// M10 (M10_TASK.md §7): on a fatal error, stop accepting new work but
    /// **leave the output session, recorder, grab, and fd available** for
    /// the caller's unified ordered shutdown (the immediate fail-open is
    /// preserved for [`EvdevRuntime::step`]).
    fn defer_or_fail_open(&mut self, error: RuntimeError, defer: bool) -> RuntimeError {
        if defer {
            if self.phase == RuntimePhase::Running {
                self.phase = RuntimePhase::Stopping;
            }
            error
        } else {
            self.fail_open(error)
        }
    }

    /// The current lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> RuntimePhase {
        self.phase
    }

    /// The current decoder synchronization state.
    #[must_use]
    pub fn sync_state(&self) -> crate::decode::SyncState {
        self.decoder
            .as_ref()
            .expect("the decoder is always present until into_sink")
            .sync_state()
    }

    /// The controlled shutdown lifecycle — one ordered finalization for the
    /// signal and grab-failure paths (the fatal path runs the same sequence
    /// inside [`EvdevRuntime::fail_open`], M5 re-review R3):
    ///
    /// 1. **Stop work** — the phase becomes [`RuntimePhase::Stopping`], so
    ///    subsequent [`EvdevRuntime::step`] calls are refused.
    /// 2. **End output / recorder finalization** — the semantic-output
    ///    lifecycle is an explicitly documented no-op (Phase 1 has no real
    ///    output backend); the raw-event recorder (if attached, M5) is
    ///    **completely finalized here, before the grab release**: `finish`
    ///    (which flushes) is called and the recorder is then destroyed, so
    ///    its best-effort `Drop` flush (the last chance to push buffered
    ///    bytes when `finish` failed) also precedes the device release.
    /// 3. **Idempotent ungrab** — `EVIOCGRAB(0)` at most once; repeated
    ///    shutdown is a no-op.
    /// 4. **Close fd** — after the ungrab, exactly once; repeated shutdown
    ///    is a no-op.
    ///
    /// Returns a [`ShutdownReport`] with the per-step results. Repeated
    /// shutdown, and shutdown after a fatal fail-open, are safe no-ops (the
    /// recorder was already finalized and the device already released; the
    /// report's fields are `None`/`0`).
    pub fn shutdown(&mut self) -> ShutdownReport {
        // Repeated shutdown / shutdown after a fail-open: everything already
        // happened — the recorder was finalized and the device released.
        if self.phase == RuntimePhase::Stopped {
            return ShutdownReport {
                phase: RuntimePhase::Stopped,
                recorder_finish: None,
                events_recorded: 0,
                ungrab: None,
                close: None,
            };
        }
        self.phase = RuntimePhase::Stopping;
        // Step 2: complete recorder finalization (before the ungrab). The
        // semantic-output lifecycle end remains an explicit no-op (no
        // backend this phase).
        let (recorder_finish, events_recorded) = self.finalize_recorder();
        // Steps 3/4: idempotent ungrab then close, regardless of prior
        // errors.
        let (ungrab, close) = if let Some(mut device) = self.device.take() {
            (Some(device.ungrab()), Some(device.close()))
        } else {
            (None, None)
        };
        self.phase = RuntimePhase::Stopped;
        ShutdownReport {
            phase: self.phase,
            recorder_finish,
            events_recorded,
            ungrab,
            close,
        }
    }

    /// Consumes the runtime and returns the frame sink (for tests and
    /// downstream consumers). The decoder is taken out of the runtime, so
    /// the ordered `Drop` fallback has nothing left to finalize afterwards.
    #[must_use]
    pub fn into_sink(mut self) -> S {
        self.decoder
            .take()
            .expect("the decoder is always present until into_sink")
            .into_sink()
    }

    /// Takes the frame sink out of the runtime **without consuming it** (M10:
    /// the takeover coordinator releases the virtual output session through
    /// the sink before running the ordered shutdown, which still finalizes
    /// the recorder and releases the device). Returns `None` when the decoder
    /// was already taken.
    #[must_use]
    pub fn take_sink(&mut self) -> Option<S> {
        self.decoder.take().map(|decoder| decoder.into_sink())
    }

    /// A mutable reference to the decoder's frame sink (M10: the takeover
    /// coordinator prepares a streaming output session through the sink after
    /// the device is open but before any read or grab). Returns `None` when
    /// the decoder was already taken.
    #[must_use]
    pub fn sink_mut(&mut self) -> Option<&mut S> {
        self.decoder.as_mut().map(|decoder| decoder.sink_mut())
    }

    /// The open session fd, if the device is still held (M10: the bounded
    /// event loop polls this fd for readiness).
    #[must_use]
    pub fn fd(&self) -> Option<Fd> {
        self.device.as_ref().and_then(DeviceHandle::fd)
    }

    /// Fail-open cleanup for a fatal stream/decoder/recorder error (M4
    /// immediate fail-open preserved): run the **same ordered finalization
    /// as the controlled shutdown** — recorder `finish` plus best-effort
    /// recorder destruction, then ungrab at most once, then close regardless
    /// of prior errors (M5 re-review R3) — record every step's result in the
    /// fail-open report, mark the runtime stopped, and return `error`
    /// unchanged.
    ///
    /// The recorder's complete fallible finalization (including its
    /// best-effort `Drop` destruction, needed to flush buffered bytes when
    /// `finish` fails) happens **before** the grab release, so a trace never
    /// loses recorded events on a fatal path and the fallible `finish` is
    /// never postponed past the device release.
    fn fail_open(&mut self, error: RuntimeError) -> RuntimeError {
        self.phase = RuntimePhase::Stopped;
        // Record every cleanup step's result (M5 review R3): a fatal
        // stream/decoder/recorder error must not silently discard recorder
        // finalization, ungrab, or close failures — the caller observes them
        // through [`EvdevRuntime::take_fail_open_report`].
        let (recorder_finish, events_recorded) = self.finalize_recorder();
        let (ungrab, close) = if let Some(mut device) = self.device.take() {
            (Some(device.ungrab()), Some(device.close()))
        } else {
            (None, None)
        };
        self.fail_open_report = Some(ShutdownReport {
            phase: RuntimePhase::Stopped,
            recorder_finish,
            events_recorded,
            ungrab,
            close,
        });
        error
    }

    /// Takes the cleanup report of the most recent fail-open (M5 review R3),
    /// if any. The caller (e.g. the record command's finalization) merges it
    /// into the structured result so no cleanup failure is lost on the fatal
    /// stream path.
    #[must_use]
    pub fn take_fail_open_report(&mut self) -> Option<ShutdownReport> {
        self.fail_open_report.take()
    }

    /// Completes the recorder's fallible finalization **before the device
    /// release** (M5 re-review R3): takes the recorder out, calls its
    /// `finish` (which flushes), captures the recorded-event count for
    /// status reporting, and destroys the recorder so its best-effort `Drop`
    /// flush — the last chance to push buffered bytes when `finish` failed —
    /// also happens here, strictly before the ungrab and close that the
    /// caller performs next. The caller can therefore never call the
    /// fallible `finish` after the device was released, and the report
    /// carries the actual finalization result.
    ///
    /// Returns `(finish result, events recorded)`; `(None, 0)` when no
    /// recorder is attached.
    fn finalize_recorder(&mut self) -> (Option<Result<(), RecorderError>>, u64) {
        let mut recorder = match self.recorder.take() {
            Some(recorder) => recorder,
            None => return (None, 0),
        };
        let events_recorded = recorder.events_recorded();
        let finish = recorder.finish();
        // Best-effort recorder destruction: its `Drop` flushes whatever
        // buffered bytes a failed `finish` could not push — before the
        // device release below.
        drop(recorder);
        (Some(finish), events_recorded)
    }

    /// Records every decoded event of the current read batch **before** the
    /// decoder feeds (M5). Takes the recorder out temporarily so the
    /// fail-open cleanup can run without a self-borrow conflict.
    fn record_all(&mut self, events: &[KernelEvent]) -> Result<(), RecorderError> {
        let mut recorder = match self.recorder.take() {
            Some(recorder) => recorder,
            None => return Ok(()),
        };
        let result = events.iter().try_for_each(|event| recorder.record(event));
        self.recorder = Some(recorder);
        result
    }

    /// Whether a stop was requested (M5): either the attached stop flag is
    /// set (an injectable stop source, used by tests and future non-signal
    /// stops) or the process-lifetime termination handler recorded a request
    /// ([`crate::signals::termination_requested`], M5 re-review R1 — the
    /// handler sets that static instead of a caller-owned flag, so the
    /// runtime observes real `SIGINT`/`SIGTERM` deliveries without any
    /// caller allocation on the async handler path).
    fn stop_requested(&self) -> bool {
        let attached = self
            .stop_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed));
        attached || crate::signals::termination_requested()
    }
}

/// Ordered best-effort fallback destruction (M5 review R4, extended by R3).
///
/// The explicit [`EvdevRuntime::shutdown`]/[`EvdevRuntime::fail_open`] paths
/// are primary and mark the runtime [`RuntimePhase::Stopped`]. This `Drop`
/// only covers fallback destruction of a runtime that never reached
/// `Stopped` — early `?` returns after the recorder was attached (e.g. the
/// record command's status-writer failure) and unexpected unwinds. It
/// completes the recorder's fallible finalization (`finish`, best-effort)
/// **and destroys the recorder** before the `DeviceHandle` field is dropped,
/// so both the `finish` and the recorder's best-effort `Drop` flush always
/// precede the device release (ungrab then close, each at most once via
/// [`DeviceHandle`]'s own `Drop`) — the field ordering can never finalize
/// the recorder after the device was released.
impl<S: FrameSink> Drop for EvdevRuntime<S> {
    fn drop(&mut self) {
        if self.phase != RuntimePhase::Stopped {
            // Best-effort (errors are not reportable from `Drop`), but
            // ordered: finish + recorder destruction happen here, before the
            // `device` field's `Drop` runs below.
            let _ = self.finalize_recorder();
        }
        // The `device` field's `Drop` then best-effort ungrabs (at most
        // once) and closes the fd — after the recorder finalization above.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    use touchpad_core::ContactState;

    use crate::codes::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, EV_ABS, EV_SYN,
        SYN_DROPPED, SYN_REPORT,
    };
    use crate::event::KernelEvent;
    use crate::recorder::{RawEventRecorder, RecorderError};
    use crate::sink::RecordingFrameSink;
    use crate::sys::mock::{MockCall, MockDevice, MockFailure, MockSys};
    use crate::sys::{AbsInfo, Fd, InputId, SysError};

    fn mock_touchpad(_sec: i64) -> MockDevice {
        let mut device = MockDevice::touchpad("Pad", 10);
        device.mt_slots.insert(ABS_MT_TRACKING_ID, vec![-1; 10]);
        device.mt_slots.insert(ABS_MT_POSITION_X, vec![0; 10]);
        device.mt_slots.insert(ABS_MT_POSITION_Y, vec![0; 10]);
        device
    }

    fn open_runtime(
        sys: &Rc<MockSys>,
        path: &Path,
        grab: bool,
    ) -> EvdevRuntime<RecordingFrameSink> {
        let sys: Rc<dyn Sys> = sys.clone();
        let mut runtime = EvdevRuntime::open(sys, path, RecordingFrameSink::new()).unwrap();
        // `open` never grabs (M5 review R2): the tests that want a grab
        // request it explicitly through the checked runtime interface.
        if grab {
            runtime.grab().unwrap();
        }
        runtime
    }

    /// Helper: expect `open` to fail and return the error (avoids a `Debug`
    /// bound on `EvdevRuntime`). `open` never grabs (M5 review R2).
    fn open_err(sys: &Rc<MockSys>, path: &Path) -> RuntimeError {
        let sys: Rc<dyn Sys> = sys.clone();
        match EvdevRuntime::open(sys, path, RecordingFrameSink::new()) {
            Err(error) => error,
            Ok(_) => panic!("expected open to fail"),
        }
    }

    fn ev(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
        event::encode_input_event(sec, usec, event_type, code, value)
    }

    /// Pushes a batch of raw events as a single read chunk (one `step`
    /// consumes one read).
    fn push_batch(device: &mut MockDevice, events: Vec<Vec<u8>>) {
        device.push_raw(events.concat());
    }

    /// The runtime's fd: the one that was grabbed (the probe's temporary fd
    /// is distinct and should not be counted by cleanup assertions).
    fn runtime_fd(sys: &MockSys) -> crate::sys::Fd {
        match sys
            .log()
            .iter()
            .find(|call| matches!(call, MockCall::Grab(_, true)))
        {
            Some(MockCall::Grab(fd, true)) => *fd,
            _ => panic!("no grab recorded; did the runtime grab the device?"),
        }
    }

    fn begin_contact(sec: i64) -> Vec<Vec<u8>> {
        vec![
            ev(sec, 0, EV_ABS, ABS_MT_SLOT, 0),
            ev(sec, 0, EV_ABS, ABS_MT_TRACKING_ID, 7),
            ev(sec, 0, EV_ABS, ABS_MT_POSITION_X, 100),
            ev(sec, 0, EV_ABS, ABS_MT_POSITION_Y, 50),
            ev(sec, 0, EV_SYN, SYN_REPORT, 0),
        ]
    }

    #[test]
    fn open_rejects_non_candidate_with_reasons() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Not MT", 10);
        device.abs_bits[ABS_MT_TRACKING_ID as usize / 8] &= !(1 << (ABS_MT_TRACKING_ID % 8));
        device.absinfo.remove(&ABS_MT_TRACKING_ID);
        sys.add_device(&path, device);
        let err = open_err(&sys, &path);
        assert!(matches!(
            err,
            RuntimeError::Open(OpenError::NotCandidate { .. })
        ));
        assert!(err.to_string().contains("not Type-B multitouch"), "{err}");
    }

    #[test]
    fn open_permission_denied_is_actionable() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.set_open_error(&path, MockFailure::PermissionDenied);
        let err = open_err(&sys, &path);
        assert!(
            matches!(err, RuntimeError::Open(OpenError::Access { .. })),
            "{err:?}"
        );
        assert!(err.to_string().contains("permission"), "{err}");
    }

    #[test]
    fn open_of_missing_device_is_actionable() {
        let sys = Rc::new(MockSys::new());
        let err = open_err(&sys, Path::new("/dev/input/event0"));
        assert!(matches!(err, RuntimeError::Open(OpenError::Access { .. })));
        assert!(err.to_string().contains("no such file"), "{err}");
    }

    #[test]
    fn step_decodes_events_into_frames() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, false);
        let fed = runtime.step().unwrap();
        assert_eq!(fed, 5);
        let frames = runtime.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].contacts.len(), 1);
        assert_eq!(frames[0].contacts[0].tracking_id, 7);
        assert_eq!(frames[0].contacts[0].state, ContactState::Began);
    }

    /// M4 review R1: the runtime must request `CLOCK_MONOTONIC` on its
    /// session fd via `EVIOCSCLOCKID` **before** the grab and **before** any
    /// read — evdev defaults to `INPUT_CLK_REAL`, so monotonic timestamps
    /// are not by construction.
    #[test]
    fn open_sets_monotonic_clock_before_grab_and_read() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let fd = runtime_fd(&sys);
        // Exactly one EVIOCSCLOCKID(CLOCK_MONOTONIC) on the runtime's fd.
        assert_eq!(
            sys.count(|call| matches!(
                call,
                MockCall::ClockId(f, clock) if *f == fd && *clock == crate::sys::CLOCK_MONOTONIC
            )),
            1
        );
        let log = sys.log();
        let clock = log
            .iter()
            .position(|call| matches!(call, MockCall::ClockId(f, _) if *f == fd))
            .expect("clock ioctl");
        let grab = log
            .iter()
            .position(|call| matches!(call, MockCall::Grab(f, true) if *f == fd))
            .expect("grab");
        assert!(clock < grab, "clock must be selected before the grab");
        // The runtime also set the clock before any read happens.
        runtime.step().unwrap();
        let log = sys.log();
        let read = log
            .iter()
            .position(|call| matches!(call, MockCall::Read(f, _) if *f == fd))
            .expect("read");
        assert!(clock < read, "clock must be selected before any read");
    }

    /// M4 review R1: a failed `EVIOCSCLOCKID` is an actionable setup error —
    /// the fd is closed, and the runtime never grabs or reads.
    #[test]
    fn clock_failure_closes_fd_and_never_grabs_or_reads() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.clock_id_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let err = open_err(&sys, &path);
        assert!(
            matches!(err, RuntimeError::Open(OpenError::Clock { .. })),
            "{err:?}"
        );
        assert!(err.to_string().contains("CLOCK_MONOTONIC"), "{err}");
        // The session fd was closed; no grab and no read ever happened.
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
        assert_eq!(sys.count(|call| matches!(call, MockCall::Read(..))), 0);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(_))),
            1,
            "the session fd must be closed exactly once"
        );
    }

    /// M4 review R4: a runtime session opens the device exactly once, and
    /// every capability/axis/slot query runs on that same fd (no probe-then-
    /// reopen, so a path swap cannot attach one device's validation to
    /// another device).
    #[test]
    fn open_validates_and_runs_the_same_fd() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let _runtime = open_runtime(&sys, &path, true);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Open(_))),
            1,
            "a runtime session must open the device exactly once"
        );
        let fd = runtime_fd(&sys);
        // Every query ioctl, the clock selection, and the close all target
        // the same session fd.
        for call in sys.log() {
            match call {
                MockCall::Name(f)
                | MockCall::Id(f)
                | MockCall::EvBits(f, _)
                | MockCall::PropBits(f)
                | MockCall::AbsInfo(f, _)
                | MockCall::ClockId(f, _)
                | MockCall::Grab(f, _)
                | MockCall::Close(f) => {
                    assert_eq!(f, fd, "all session queries must use the session fd")
                }
                _ => {}
            }
        }
    }

    /// M4 review R4 + M5 review R2: the optional grab is issued after the
    /// clock selection and decoder/snapshot preparation (via the checked
    /// [`EvdevRuntime::grab`], since `open` no longer grabs) and before the
    /// first read; a grab failure leaves the device open for the caller's
    /// cleanup (shutdown closes the fd exactly once) and no read ever
    /// happens.
    #[test]
    fn grab_is_issued_after_preparation_and_failure_cleans_up() {
        // Successful grab: it must come after the clock selection and before
        // the first read.
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        assert!(runtime.is_grabbed());
        let fd = runtime_fd(&sys);
        let log = sys.log();
        let clock = log
            .iter()
            .position(|call| matches!(call, MockCall::ClockId(f, _) if *f == fd))
            .unwrap();
        let grab = log
            .iter()
            .position(|call| matches!(call, MockCall::Grab(f, true) if *f == fd))
            .unwrap();
        assert!(clock < grab, "clock must be selected before the grab");
        runtime.step().unwrap();
        let log = sys.log();
        let read = log
            .iter()
            .position(|call| matches!(call, MockCall::Read(f, _) if *f == fd))
            .unwrap();
        assert!(grab < read, "the grab must precede the first read");

        // Grab failure: `open` still succeeds (it never grabs), `grab()`
        // fails, the fd stays open for the caller's cleanup (shutdown closes
        // it exactly once), and no read ever happens.
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.grab_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let sys_rc: Rc<dyn Sys> = sys.clone();
        let mut runtime = EvdevRuntime::open(sys_rc, &path, RecordingFrameSink::new()).unwrap();
        let err = runtime.grab().unwrap_err();
        assert!(matches!(err, RuntimeError::Grab(_)), "{err:?}");
        assert!(!runtime.is_grabbed());
        assert_eq!(sys.count(|call| matches!(call, MockCall::Read(..))), 0);
        let report = runtime.shutdown();
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(_))),
            1,
            "the session fd must be closed exactly once after a failed grab"
        );
    }

    #[test]
    fn eof_releases_grab_closes_and_stops() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let mut runtime = open_runtime(&sys, &path, true);
        // Empty read stream -> EOF on the first step.
        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::DeviceGone));
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let fd = runtime_fd(&sys);
        // Fail-open cleanup: ungrab then close the runtime's fd, exactly
        // once each (the probe's own open/close is a different fd).
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, true) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        // Subsequent steps are refused; shutdown is idempotent.
        assert!(matches!(runtime.step(), Err(RuntimeError::NotRunning)));
        let report = runtime.shutdown();
        assert_eq!(report.phase, RuntimePhase::Stopped);
        assert!(report.ungrab.is_none());
        assert!(report.close.is_none());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    /// M10 (M10_TASK.md §7): `step_deferred` stops accepting new work on a
    /// fatal stream error but **leaves the output, recorder, grab, and fd
    /// available** for the caller's unified ordered shutdown — unlike
    /// `step`, which fails open immediately. The coordinator then runs the
    /// ordered `shutdown` (recorder finalize → ungrab → close) exactly once.
    #[test]
    fn step_deferred_leaves_resources_for_the_ordered_shutdown() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let mut runtime = open_runtime(&sys, &path, true);
        // Empty read stream -> EOF on the first deferred step.
        let err = runtime.step_deferred().unwrap_err();
        assert!(matches!(err, RuntimeError::DeviceGone));
        // The runtime stopped accepting new work...
        assert_eq!(runtime.phase(), RuntimePhase::Stopping);
        assert!(matches!(
            runtime.step_deferred(),
            Err(RuntimeError::NotRunning)
        ));
        // ... but the device is still open and the grab still held: the
        // caller's ordered shutdown performs the release.
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            0
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            0
        );
        let report = runtime.shutdown();
        assert_eq!(report.phase, RuntimePhase::Stopped);
        // The ordered shutdown released the grab and closed the fd exactly
        // once each.
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        // Repeated shutdown is a full no-op.
        let report = runtime.shutdown();
        assert!(report.ungrab.is_none());
        assert!(report.close.is_none());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    /// M10: a decoder failure (resync failure) on the deferred path leaves
    /// the recorder attached and the device open, so the coordinator can
    /// finalize the recorder and release the device in order.
    #[test]
    fn step_deferred_decoder_failure_keeps_the_recorder_for_finalize() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::new(RefCell::new(Vec::new())),
            events: 0,
        }));
        let err = runtime.step_deferred().unwrap_err();
        assert!(matches!(err, RuntimeError::Decode(_)), "{err}");
        assert_eq!(runtime.phase(), RuntimePhase::Stopping);
        // The recorder is still attached (the deferred path did not finalize
        // it) — the coordinator's ordered shutdown finalizes it.
        assert!(runtime.recorder().is_some(), "recorder must stay attached");
        let report = runtime.shutdown();
        assert!(
            report.recorder_finish.is_some(),
            "the ordered shutdown must finalize the recorder"
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    #[test]
    fn partial_read_is_fatal_and_actionable() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.push_raw(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]); // torn
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::PartialRead { actual: 10, .. }));
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    #[test]
    fn einterrupt_is_actionable_and_releases() {
        // EINTR-without-stop semantics depend on the process-lifetime signal
        // static being false; serialize with signal tests that mutate it.
        let _signal_lock = crate::signals::SIGNAL_TEST_LOCK.lock().unwrap();
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.push_read_failure(MockFailure::Interrupted);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::Read(SysError::Interrupted)));
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    #[test]
    fn timestamp_regression_is_fatal() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(2);
        // First event at sec 2, then the stream regresses to sec 1.
        push_batch(
            &mut device,
            vec![
                ev(2, 0, EV_SYN, SYN_REPORT, 0),
                ev(1, 0, EV_SYN, SYN_REPORT, 0),
            ],
        );
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::TimestampRegression { previous, current }
                if previous.as_nanos() > current.as_nanos()
        ));
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
    }

    #[test]
    fn invalid_timeval_is_fatal() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.push_raw(ev(-1, 0, EV_SYN, SYN_REPORT, 0)); // negative sec
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::Timeval(TimevalError::NegativeSeconds(-1))
        ));
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
    }

    #[test]
    fn resync_failure_degrades_releases_grab_and_stops_frames() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The resync snapshot query fails.
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(
            matches!(err, RuntimeError::Decode(DecodeError::ResyncFailed(_))),
            "{err:?}"
        );
        // The decoder is degraded and the grab was released.
        assert_eq!(runtime.sync_state(), crate::decode::SyncState::Degraded);
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        // No trusted frame was published for the failed resync.
        let frames = runtime.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 1, "only the pre-drop frame may be published");
        assert!(!frames[0].discontinuity);
    }

    #[test]
    fn successful_resync_publishes_a_discontinuity_frame() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The kernel snapshot sees slot 0 active with tracking id 42.
        device.mt_slots.insert(
            ABS_MT_TRACKING_ID,
            vec![42, -1, -1, -1, -1, -1, -1, -1, -1, -1],
        );
        device
            .mt_slots
            .insert(ABS_MT_POSITION_X, vec![250, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        device
            .mt_slots
            .insert(ABS_MT_POSITION_Y, vec![125, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, false);
        runtime.step().unwrap();
        assert_eq!(runtime.sync_state(), crate::decode::SyncState::Normal);
        let frames = runtime.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 2);
        assert!(frames[1].discontinuity);
        assert_eq!(frames[1].contacts.len(), 1);
        assert_eq!(frames[1].contacts[0].tracking_id, 42);
        assert_eq!(frames[1].contacts[0].state, ContactState::Began);
    }

    #[test]
    fn shutdown_is_idempotent_and_releases_in_order() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        runtime.step().unwrap();
        let fd = runtime_fd(&sys);
        let report = runtime.shutdown();
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());
        assert_eq!(report.phase, RuntimePhase::Stopped);
        // Repeated shutdown: no further syscalls.
        let report = runtime.shutdown();
        assert!(report.ungrab.is_none());
        assert!(report.close.is_none());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        let log = sys.log();
        let ungrab = log
            .iter()
            .position(|c| matches!(c, MockCall::Grab(f, false) if *f == fd))
            .unwrap();
        let close = log
            .iter()
            .position(|c| matches!(c, MockCall::Close(f) if *f == fd))
            .unwrap();
        assert!(ungrab < close, "ungrab must precede close");
        assert!(matches!(runtime.step(), Err(RuntimeError::NotRunning)));
    }

    /// M4 review R5: an explicit shutdown whose `EVIOCGRAB(0)` fails still
    /// reports the first ungrab error, closes the fd (fail-open), and never
    /// issues a second release ioctl — including on repeated shutdown.
    #[test]
    fn shutdown_with_failed_ungrab_reports_error_and_releases_once() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let fd = runtime_fd(&sys);
        let report = runtime.shutdown();
        // The first ungrab error is preserved while close still succeeds.
        assert!(matches!(report.ungrab, Some(Err(GrabError::Io(_)))));
        assert!(report.close.as_ref().unwrap().is_ok());
        assert_eq!(report.phase, RuntimePhase::Stopped);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1,
            "a failed release must be attempted exactly once"
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        // Repeated shutdown is a safe no-op: no new syscalls.
        let report = runtime.shutdown();
        assert!(report.ungrab.is_none());
        assert!(report.close.is_none());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    /// M4 review R5: a fatal decoder/resync error whose `EVIOCGRAB(0)` also
    /// fails still performs exactly one release attempt and one close
    /// (fail-open), and drops the runtime's remaining frames.
    #[test]
    fn fatal_resync_cleanup_with_failed_ungrab_releases_once_and_closes() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The resync snapshot query fails AND the release fails.
        device.mt_slots_error = Some(MockFailure::Io);
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(
            matches!(err, RuntimeError::Decode(DecodeError::ResyncFailed(_))),
            "{err:?}"
        );
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1,
            "a failed release must be attempted exactly once on the fatal path"
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let frames = runtime.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 1, "only the pre-drop frame may be published");
    }

    /// M4 review R6 regression: one read batch contains a normal frame,
    /// `SYN_DROPPED`, the recovery `SYN_REPORT`, and then multiple
    /// post-boundary tracking-id lifecycles. The recovery snapshot already
    /// includes those later events, so the runtime must drain the rest of
    /// the batch: no stale lifecycle or frame may be emitted after the
    /// discontinuity frame.
    #[test]
    fn resync_drains_the_rest_of_the_read_batch() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1); // slot 0, tid 7 -> frame 1
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        // An incremental event inside the dropped window (ignored).
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 999));
        // The recovery boundary: the snapshot is taken here.
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        // Post-boundary stale lifecycles that predate the snapshot ioctl:
        // slot 0 tid 2 begin/end, slot 0 tid 3 begin, slot 1 tid 4 begin,
        // ending in a SYN_REPORT that would commit them.
        batch.push(ev(1, 0, EV_ABS, ABS_MT_SLOT, 0));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 2));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, -1));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 3));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 300));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_Y, 150));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_SLOT, 1));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 4));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 100));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_Y, 50));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The snapshot sees slot 0 active with tid 3 and slot 1 empty.
        device.mt_slots.insert(
            ABS_MT_TRACKING_ID,
            vec![3, -1, -1, -1, -1, -1, -1, -1, -1, -1],
        );
        device
            .mt_slots
            .insert(ABS_MT_POSITION_X, vec![300, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        device
            .mt_slots
            .insert(ABS_MT_POSITION_Y, vec![150, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, false);
        // 5 pre-drop events + SYN_DROPPED + ignored X + recovery SYN_REPORT.
        let fed = runtime.step().unwrap();
        assert_eq!(
            fed, 8,
            "feeding must stop right after the recovery boundary"
        );
        assert_eq!(runtime.sync_state(), crate::decode::SyncState::Normal);
        let frames = runtime.into_sink().frames().to_vec();
        // Exactly two frames: the pre-drop normal frame and the discontinuity
        // frame from the snapshot. The drained batch must produce nothing.
        assert_eq!(frames.len(), 2, "{frames:#?}");
        assert!(!frames[0].discontinuity);
        assert_eq!(frames[0].contacts[0].tracking_id, 7);
        assert!(frames[1].discontinuity);
        assert_eq!(frames[1].contacts.len(), 1);
        assert_eq!(frames[1].contacts[0].tracking_id, 3);
        // No stale lifecycle from the drained post-boundary events.
        for frame in &frames {
            for contact in &frame.contacts {
                assert_ne!(
                    contact.tracking_id, 2,
                    "stale tid 2 lifecycle must not be emitted"
                );
                assert_ne!(
                    contact.tracking_id, 4,
                    "stale tid 4 lifecycle must not be emitted"
                );
            }
        }
    }

    /// M4 review R7: a truncated `EVIOCGKEY` response during resync (shorter
    /// than the bytes covering `BTN_LEFT..BTN_MIDDLE`) must fail the
    /// snapshot closed: the decoder degrades, no discontinuity frame is
    /// published, and the runtime fail-opens (release + close).
    #[test]
    fn short_key_state_during_resync_fails_closed_with_no_frame() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.add_key(crate::BTN_LEFT);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // 34 bytes covers BTN_LEFT/BTN_RIGHT but not BTN_MIDDLE (byte 34).
        device.key_state = vec![0u8; 34];
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        let err = runtime.step().unwrap_err();
        assert!(
            matches!(err, RuntimeError::Decode(DecodeError::ResyncFailed(_))),
            "{err:?}"
        );
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        let frames = runtime.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 1, "only the pre-drop frame may be published");
        assert!(!frames[0].discontinuity);
    }

    #[test]
    fn drop_releases_grab_as_fallback() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let fd;
        {
            let _runtime = open_runtime(&sys, &path, true);
            fd = runtime_fd(&sys);
        }
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    // ---------------------------------------------------------------------
    // M5: raw-event recorder and signal stop
    // ---------------------------------------------------------------------

    /// A recorder that records markers into a shared timeline, so tests can
    /// interleave recorder activity with the syscall log.
    struct MarkerRecorder {
        timeline: Rc<RefCell<Vec<String>>>,
        events: u64,
    }

    impl RawEventRecorder for MarkerRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
            self.timeline
                .borrow_mut()
                .push("recorder:record".to_string());
            self.events += 1;
            Ok(())
        }

        fn flush(&mut self) -> Result<(), RecorderError> {
            self.timeline
                .borrow_mut()
                .push("recorder:flush".to_string());
            Ok(())
        }

        fn finish(&mut self) -> Result<(), RecorderError> {
            self.timeline
                .borrow_mut()
                .push("recorder:finish".to_string());
            Ok(())
        }

        fn events_recorded(&self) -> u64 {
            self.events
        }
    }

    /// The recorder's best-effort destruction is part of the ordered
    /// finalization (M5 re-review R3): its `Drop` runs before the device
    /// release, so tests can assert the destruction precedes the ungrab.
    impl Drop for MarkerRecorder {
        fn drop(&mut self) {
            self.timeline.borrow_mut().push("recorder:drop".to_string());
        }
    }

    /// A recorder whose `record` always fails.
    struct FailingRecorder;

    impl RawEventRecorder for FailingRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
            Err(RecorderError::Trace(
                touchpad_trace::TraceError::InvalidState("injected recorder failure"),
            ))
        }

        fn flush(&mut self) -> Result<(), RecorderError> {
            Ok(())
        }

        fn finish(&mut self) -> Result<(), RecorderError> {
            Ok(())
        }

        fn events_recorded(&self) -> u64 {
            0
        }
    }

    /// A sys seam that records a marker for every call into a shared
    /// timeline, delegating behavior to a `MockSys`. Used to prove the
    /// shutdown ordering (recorder finalization → ungrab → close) in one
    /// timeline.
    struct TimelineSys {
        inner: Rc<MockSys>,
        timeline: Rc<RefCell<Vec<String>>>,
    }

    impl crate::sys::Sys for TimelineSys {
        fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("read_dir({})", path.display()));
            self.inner.read_dir(path)
        }

        fn open(&self, path: &Path) -> Result<Fd, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("open({})", path.display()));
            self.inner.open(path)
        }

        fn close(&self, fd: Fd) -> Result<(), SysError> {
            self.timeline.borrow_mut().push(format!("close({fd:?})"));
            self.inner.close(fd)
        }

        fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
            self.timeline.borrow_mut().push(format!("read({fd:?})"));
            self.inner.read(fd, buf)
        }

        fn ioctl_grab(&self, fd: Fd, grab: bool) -> Result<(), SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("grab({fd:?}, {grab})"));
            self.inner.ioctl_grab(fd, grab)
        }

        fn ioctl_set_clock_id(&self, fd: Fd, clock_id: u32) -> Result<(), SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("clock({fd:?}, {clock_id})"));
            self.inner.ioctl_set_clock_id(fd, clock_id)
        }

        fn ioctl_name(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
            self.timeline.borrow_mut().push(format!("name({fd:?})"));
            self.inner.ioctl_name(fd, buf)
        }

        fn ioctl_id(&self, fd: Fd) -> Result<InputId, SysError> {
            self.timeline.borrow_mut().push(format!("id({fd:?})"));
            self.inner.ioctl_id(fd)
        }

        fn ioctl_ev_bits(&self, fd: Fd, ev_type: u16, buf: &mut [u8]) -> Result<usize, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("evbits({fd:?}, {ev_type})"));
            self.inner.ioctl_ev_bits(fd, ev_type, buf)
        }

        fn ioctl_prop_bits(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
            self.timeline.borrow_mut().push(format!("propbits({fd:?})"));
            self.inner.ioctl_prop_bits(fd, buf)
        }

        fn ioctl_key_state(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
            self.timeline.borrow_mut().push(format!("keystate({fd:?})"));
            self.inner.ioctl_key_state(fd, buf)
        }

        fn ioctl_absinfo(&self, fd: Fd, abs_code: u16) -> Result<AbsInfo, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("absinfo({fd:?}, {abs_code})"));
            self.inner.ioctl_absinfo(fd, abs_code)
        }

        fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError> {
            self.timeline.borrow_mut().push(format!("mtslots({fd:?})"));
            self.inner.ioctl_mt_slots(fd, buf)
        }

        fn poll(&self, fd: Fd, timeout: std::time::Duration) -> Result<bool, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("poll({fd:?}, {timeout:?})"));
            self.inner.poll(fd, timeout)
        }
    }

    /// M5: an `EINTR` from the blocked read with the stop flag set is a
    /// **graceful stop**, not an ordinary fatal error: the runtime stops
    /// accepting new work but leaves the device open so the caller can run
    /// the ordered shutdown.
    #[test]
    fn eintr_with_stop_flag_is_graceful_and_leaves_the_device_open() {
        // The runtime's stop check observes the process-lifetime signal
        // static, which signal tests mutate — serialize with them.
        let _signal_lock = crate::signals::SIGNAL_TEST_LOCK.lock().unwrap();
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.push_read_failure(MockFailure::Interrupted);
        sys.add_device(&path, device);
        let mut runtime = open_runtime(&sys, &path, true);
        runtime.set_stop_flag(Arc::new(AtomicBool::new(true)));

        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::Interrupted), "{err:?}");
        // The runtime stopped accepting new work but did NOT fail open.
        assert_eq!(runtime.phase(), RuntimePhase::Stopping);
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            0,
            "no ungrab may happen before the ordered shutdown"
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            0
        );

        // The caller then runs the ordered shutdown.
        let report = runtime.shutdown();
        assert!(report.recorder_finish.is_none()); // no recorder attached
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());
        assert_eq!(report.phase, RuntimePhase::Stopped);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    /// M5: the signal-stop shutdown order is recorder finalization
    /// (finish + destruction) → ungrab → close, all observable in one
    /// shared timeline.
    #[test]
    fn signal_stop_orders_finish_before_ungrab_before_close() {
        // The runtime's stop check observes the process-lifetime signal
        // static, which signal tests mutate — serialize with them.
        let _signal_lock = crate::signals::SIGNAL_TEST_LOCK.lock().unwrap();
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn crate::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        device.push_read_failure(MockFailure::Interrupted);
        mock.add_device(&path, device);

        let mut runtime =
            EvdevRuntime::open(Rc::clone(&sys), &path, RecordingFrameSink::new()).unwrap();
        runtime.grab().unwrap();
        runtime.set_stop_flag(Arc::new(AtomicBool::new(true)));
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::clone(&timeline),
            events: 0,
        }));

        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::Interrupted), "{err:?}");
        assert_eq!(runtime.phase(), RuntimePhase::Stopping);

        let report = runtime.shutdown();
        assert!(report.recorder_finish.as_ref().unwrap().is_ok());
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());

        // Order in the shared timeline: recorder finish < recorder drop
        // (best-effort destruction) < ungrab < close.
        let timeline = timeline.borrow();
        let finish = timeline
            .iter()
            .position(|marker| marker == "recorder:finish")
            .expect("recorder finish in timeline");
        let drop = timeline
            .iter()
            .position(|marker| marker == "recorder:drop")
            .expect("recorder drop in timeline");
        let ungrab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", false)"))
            .expect("ungrab in timeline");
        let close = timeline
            .iter()
            .position(|marker| marker.starts_with("close("))
            .expect("close in timeline");
        assert!(
            finish < ungrab,
            "recorder finish must precede ungrab: {timeline:?}"
        );
        assert!(
            finish < drop && drop < ungrab,
            "recorder destruction must precede ungrab: {timeline:?}"
        );
        assert!(ungrab < close, "ungrab must precede close: {timeline:?}");
    }

    /// M5: the recorder is called for **every** event of a read batch before
    /// the decoder sees it — including events the decoder later drains after
    /// a successful resync (the trace is the ground truth of the raw stream).
    #[test]
    fn recorder_records_every_event_before_feed_including_drained_ones() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 999));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        // Post-boundary stale lifecycles (drained by the decoder).
        batch.push(ev(1, 0, EV_ABS, ABS_MT_SLOT, 0));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 2));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, -1));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 3));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 300));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_Y, 150));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_SLOT, 1));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 4));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_X, 100));
        batch.push(ev(1, 0, EV_ABS, ABS_MT_POSITION_Y, 50));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        device.mt_slots.insert(
            ABS_MT_TRACKING_ID,
            vec![3, -1, -1, -1, -1, -1, -1, -1, -1, -1],
        );
        device
            .mt_slots
            .insert(ABS_MT_POSITION_X, vec![300, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        device
            .mt_slots
            .insert(ABS_MT_POSITION_Y, vec![150, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        sys.add_device(&path, device);

        let timeline = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = open_runtime(&sys, &path, false);
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::clone(&timeline),
            events: 0,
        }));

        let fed = runtime.step().unwrap();
        // The decoder drains after the recovery boundary...
        assert_eq!(fed, 8);
        // ...but the recorder captured every event of the batch (19 total).
        let recorded = timeline
            .borrow()
            .iter()
            .filter(|marker| *marker == "recorder:record")
            .count();
        assert_eq!(recorded, 19, "the recorder must see every read event");
        assert_eq!(runtime.recorder().unwrap().events_recorded(), 19);
    }

    /// M5: a decoder failure (here: a failed resync) must not lose the raw
    /// events already read — the recorder recorded them before the feed that
    /// failed.
    #[test]
    fn decoder_failure_keeps_already_recorded_events() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The resync snapshot query fails -> the decoder degrades on the
        // recovery SYN_REPORT.
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);

        let timeline = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = open_runtime(&sys, &path, true);
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::clone(&timeline),
            events: 0,
        }));

        let err = runtime.step().unwrap_err();
        assert!(
            matches!(err, RuntimeError::Decode(DecodeError::ResyncFailed(_))),
            "{err:?}"
        );
        // All 7 events of the batch were recorded before the failing feed.
        let recorded = timeline
            .borrow()
            .iter()
            .filter(|marker| *marker == "recorder:record")
            .count();
        assert_eq!(recorded, 7, "no read event may be lost to a decoder bug");
        // The fatal path still fail-opens: recorder finish, ungrab, close.
        let finish = timeline
            .borrow()
            .iter()
            .position(|marker| marker == "recorder:finish")
            .expect("recorder finished on the fatal path");
        let fd = runtime_fd(&sys);
        let log = sys.log();
        let ungrab = log
            .iter()
            .position(|call| matches!(call, MockCall::Grab(f, false) if *f == fd))
            .expect("ungrab");
        let close = log
            .iter()
            .position(|call| matches!(call, MockCall::Close(f) if *f == fd))
            .expect("close");
        assert!(
            finish < ungrab,
            "recorder finish must precede ungrab on the fatal path"
        );
        assert!(
            ungrab < close,
            "ungrab must precede close on the fatal path"
        );
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
    }

    /// M5: a recorder failure is fatal for the session and still releases
    /// the grab (fail-open), with the recorder finalized before the release.
    #[test]
    fn recorder_failure_is_fatal_and_releases_the_grab() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);

        let mut runtime = open_runtime(&sys, &path, true);
        runtime.set_recorder(Box::new(FailingRecorder));

        let err = runtime.step().unwrap_err();
        assert!(matches!(err, RuntimeError::Recorder(_)), "{err:?}");
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let fd = runtime_fd(&sys);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
        // The decoder never saw the batch (the recorder failed first), so no
        // frame was produced.
        assert!(runtime.into_sink().frames().is_empty());
    }

    /// M5 (re-review R3): shutdown completes the recorder's fallible
    /// finalization — `finish` (which flushes) plus best-effort destruction —
    /// before the release, reports the finish result and the recorded-event
    /// count in the `ShutdownReport`, destroys the recorder, and repeated
    /// shutdown stays a full no-op.
    #[test]
    fn shutdown_finalizes_the_recorder_and_is_idempotent() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        sys.add_device(&path, device);

        let timeline = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = open_runtime(&sys, &path, true);
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::clone(&timeline),
            events: 0,
        }));
        runtime.step().unwrap();
        assert_eq!(runtime.recorder().unwrap().events_recorded(), 5);

        let report = runtime.shutdown();
        assert!(report.recorder_finish.as_ref().unwrap().is_ok());
        assert_eq!(report.events_recorded, 5);
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());
        // Finalization ran exactly once (finish, then destruction), before
        // the release.
        let timeline = timeline.borrow();
        assert_eq!(
            timeline.iter().filter(|m| *m == "recorder:finish").count(),
            1,
            "finish must run exactly once"
        );
        assert_eq!(
            timeline.iter().filter(|m| *m == "recorder:drop").count(),
            1,
            "the recorder must be destroyed exactly once"
        );
        let finish = timeline
            .iter()
            .position(|m| m == "recorder:finish")
            .unwrap();
        let drop_pos = timeline.iter().position(|m| m == "recorder:drop").unwrap();
        assert!(finish < drop_pos, "finish must precede destruction");
        drop(timeline);

        // Repeated shutdown: a full no-op (no recorder, no device left).
        let fd = runtime_fd(&sys);
        let report = runtime.shutdown();
        assert!(report.recorder_finish.is_none());
        assert_eq!(report.events_recorded, 0);
        assert!(report.ungrab.is_none());
        assert!(report.close.is_none());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );

        // The recorder was destroyed by finalization — the fallible `finish`
        // is called by the runtime, never by a caller after the release.
        assert!(
            runtime.into_recorder().is_none(),
            "finalization destroyed the recorder"
        );
    }

    /// M5: the runtime exposes the validated descriptor (needed by the CLI to
    /// build the trace header from the same device model the decoder uses).
    #[test]
    fn descriptor_returns_the_validated_device() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let runtime = open_runtime(&sys, &path, false);
        let descriptor = runtime.descriptor().expect("descriptor");
        assert_eq!(descriptor.name, "Pad");
        assert_eq!(descriptor.slot_count, Some(10));
        assert!(descriptor.supports_type_b_mt);
    }

    // ---------------------------------------------------------------------
    // M5 review R2/R4: checked grab interface and ordered fallback Drop
    // ---------------------------------------------------------------------

    /// A recorder whose `finish` always fails (but records nothing), used to
    /// prove that a recorder-finalization failure is reported and the device
    /// is still released in order (M5 re-review R3).
    struct FinishFailingRecorder {
        timeline: Rc<RefCell<Vec<String>>>,
    }

    impl RawEventRecorder for FinishFailingRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
            Ok(())
        }

        fn flush(&mut self) -> Result<(), RecorderError> {
            Ok(())
        }

        fn finish(&mut self) -> Result<(), RecorderError> {
            self.timeline
                .borrow_mut()
                .push("recorder:finish(fail)".to_string());
            Err(RecorderError::Trace(
                touchpad_trace::TraceError::InvalidState("injected finish failure"),
            ))
        }

        fn events_recorded(&self) -> u64 {
            0
        }
    }

    /// M5 review R2: the runtime's grab interface is checked and
    /// state-correct — it succeeds while `Running` (before any step), is
    /// idempotent (exactly one `EVIOCGRAB(1)`), and is rejected after the
    /// first step and after shutdown.
    #[test]
    fn grab_is_checked_idempotent_and_rejected_after_step_or_shutdown() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad(1));
        let mut runtime = EvdevRuntime::open(
            Rc::clone(&sys) as Rc<dyn Sys>,
            &path,
            RecordingFrameSink::new(),
        )
        .unwrap();

        // Idempotent: a second grab is a no-op (one ioctl total).
        runtime.grab().unwrap();
        assert!(runtime.is_grabbed());
        runtime.grab().unwrap();
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);

        // After the first step the grab is rejected (it must precede the
        // read loop, M5 review R2).
        push_batch(
            &mut sys.device(&path).unwrap().borrow_mut(),
            begin_contact(1),
        );
        runtime.step().unwrap();
        assert!(matches!(runtime.grab(), Err(RuntimeError::GrabAfterStep)));

        // After shutdown the grab is rejected.
        runtime.shutdown();
        assert!(matches!(runtime.grab(), Err(RuntimeError::NotRunning)));
    }

    /// M5 review R4 (extended by R3): fallback destruction (no explicit
    /// shutdown — the early `?` return / unwind path) still completes the
    /// recorder's fallible finalization — `finish` plus best-effort
    /// destruction — **before** the device release, in one shared timeline:
    /// finish < drop < ungrab < close, each device operation at most once.
    #[test]
    fn drop_finalizes_recorder_before_releasing_the_device() {
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn crate::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        mock.add_device(&path, device);

        let fd;
        {
            let mut runtime =
                EvdevRuntime::open(Rc::clone(&sys), &path, RecordingFrameSink::new()).unwrap();
            runtime.grab().unwrap();
            runtime.set_recorder(Box::new(MarkerRecorder {
                timeline: Rc::clone(&timeline),
                events: 0,
            }));
            runtime.step().unwrap();
            fd = match mock
                .log()
                .iter()
                .find(|call| matches!(call, MockCall::Grab(_, true)))
            {
                Some(MockCall::Grab(fd, true)) => *fd,
                _ => panic!("no grab recorded"),
            };
            // No explicit shutdown: the runtime is dropped as fallback.
        }

        let timeline = timeline.borrow();
        let finish = timeline
            .iter()
            .position(|marker| marker == "recorder:finish")
            .expect("recorder finish in timeline");
        let drop = timeline
            .iter()
            .position(|marker| marker == "recorder:drop")
            .expect("recorder drop in timeline");
        let ungrab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", false)"))
            .expect("ungrab in timeline");
        let close = timeline
            .iter()
            .position(|marker| marker.starts_with("close("))
            .expect("close in timeline");
        assert!(
            finish < ungrab,
            "fallback Drop must finish the recorder before the ungrab: {timeline:?}"
        );
        assert!(
            finish < drop && drop < ungrab,
            "fallback Drop must destroy the recorder before the ungrab: {timeline:?}"
        );
        assert!(
            ungrab < close,
            "fallback Drop must ungrab before close: {timeline:?}"
        );
        assert_eq!(
            mock.count(|call| matches!(call, MockCall::Grab(f, false) if *f == fd)),
            1
        );
        assert_eq!(
            mock.count(|call| matches!(call, MockCall::Close(f) if *f == fd)),
            1
        );
    }

    /// M5 re-review R3: a recorder finalization (`finish`) failure during
    /// shutdown is reported in the [`ShutdownReport`] and still releases the
    /// device in order (finish attempt < ungrab < close) — a failed finish
    /// never suppresses the device release.
    #[test]
    fn shutdown_with_finish_failure_reports_it_and_still_releases_in_order() {
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn crate::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        push_batch(&mut device, begin_contact(1));
        mock.add_device(&path, device);

        let mut runtime =
            EvdevRuntime::open(Rc::clone(&sys), &path, RecordingFrameSink::new()).unwrap();
        runtime.grab().unwrap();
        runtime.set_recorder(Box::new(FinishFailingRecorder {
            timeline: Rc::clone(&timeline),
        }));
        runtime.step().unwrap();

        let report = runtime.shutdown();
        assert!(
            matches!(report.recorder_finish, Some(Err(RecorderError::Trace(_)))),
            "{report:?}"
        );
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());

        let timeline = timeline.borrow();
        let finish = timeline
            .iter()
            .position(|marker| marker == "recorder:finish(fail)")
            .expect("finish attempt in timeline");
        let ungrab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", false)"))
            .expect("ungrab in timeline");
        let close = timeline
            .iter()
            .position(|marker| marker.starts_with("close("))
            .expect("close in timeline");
        assert!(
            finish < ungrab,
            "finish attempt must precede ungrab: {timeline:?}"
        );
        assert!(ungrab < close, "ungrab must precede close: {timeline:?}");
    }

    /// M5 re-review R3: a fatal stream/decoder error runs the **same** ordered
    /// finalization as the controlled shutdown, entirely inside fail-open —
    /// recorder finish < recorder drop (best-effort destruction) < ungrab <
    /// close in one shared timeline, with the results carried in the fail-open
    /// report. The fallible `finish` is never postponed past the release.
    #[test]
    fn fatal_path_orders_finish_before_ungrab_before_close() {
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn crate::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad(1);
        let mut batch = begin_contact(1);
        batch.push(ev(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.push(ev(1, 0, EV_SYN, SYN_REPORT, 0));
        push_batch(&mut device, batch);
        // The resync snapshot query fails -> the decoder degrades on the
        // recovery SYN_REPORT (fatal decoder error).
        device.mt_slots_error = Some(MockFailure::Io);
        mock.add_device(&path, device);

        let mut runtime =
            EvdevRuntime::open(Rc::clone(&sys), &path, RecordingFrameSink::new()).unwrap();
        runtime.grab().unwrap();
        runtime.set_recorder(Box::new(MarkerRecorder {
            timeline: Rc::clone(&timeline),
            events: 0,
        }));

        let err = runtime.step().unwrap_err();
        assert!(
            matches!(err, RuntimeError::Decode(DecodeError::ResyncFailed(_))),
            "{err:?}"
        );
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);

        let report = runtime
            .take_fail_open_report()
            .expect("fail-open must record its cleanup report");
        assert!(report.recorder_finish.as_ref().unwrap().is_ok());
        assert!(report.ungrab.as_ref().unwrap().is_ok());
        assert!(report.close.as_ref().unwrap().is_ok());

        // Order in the shared timeline: recorder finish < recorder drop <
        // ungrab < close.
        let timeline = timeline.borrow();
        let finish = timeline
            .iter()
            .position(|marker| marker == "recorder:finish")
            .expect("recorder finish on the fatal path");
        let drop = timeline
            .iter()
            .position(|marker| marker == "recorder:drop")
            .expect("recorder drop on the fatal path");
        let ungrab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", false)"))
            .expect("ungrab on the fatal path");
        let close = timeline
            .iter()
            .position(|marker| marker.starts_with("close("))
            .expect("close on the fatal path");
        assert!(
            finish < ungrab,
            "recorder finish must precede ungrab on the fatal path: {timeline:?}"
        );
        assert!(
            finish < drop && drop < ungrab,
            "recorder destruction must precede ungrab on the fatal path: {timeline:?}"
        );
        assert!(
            ungrab < close,
            "ungrab must precede close on the fatal path: {timeline:?}"
        );
    }
}
