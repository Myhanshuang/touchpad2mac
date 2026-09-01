//! # touchpad-linux
//!
//! Linux-specific raw input boundary, Type-B multitouch slot decoder, and
//! mocked resynchronization (M3), plus the Linux device boundary and
//! fail-open grab (M4).
//!
//! ## M3: decoder half of the Linux input path
//!
//! Consumes kernel-style raw events and publishes normalized
//! [`touchpad_core::ContactFrame`]s, one per `SYN_REPORT`, with an explicit
//! `Normal | DroppedAwaitingBoundary | Recovering | Degraded`
//! synchronization state machine and a mockable kernel-snapshot boundary
//! ([`ResyncSource`]) for `SYN_DROPPED` recovery.
//!
//! * [`RawEvent`] — the single input representation the decoder accepts.
//!   Live input builds it from kernel `struct input_event` values
//!   ([`crate::event`]); offline replay converts [`touchpad_trace::TraceEvent`]s into it
//!   ([`RawEvent::from_trace_event`]). Both paths feed the **exact same**
//!   [`TypeBDecoder`] state machine.
//! * [`TypeBDecoder`] — the slot state machine that commits a
//!   [`touchpad_core::ContactFrame`] per `SYN_REPORT` and owns resynchronization.
//! * [`FrameSink`] — where committed frames are published.
//!
//! ## M4: device boundary and fail-open grab
//!
//! * [`crate::sys`] — the mockable OS seam: every filesystem operation and
//!   syscall (`open`/`read`/`ioctl`/`close`, including `EVIOCGRAB`,
//!   `EVIOCSCLOCKID` and the `EVIOCG*` queries) goes through the [`Sys`]
//!   trait. The real Linux implementation ([`sys::ffi::LinuxSys`]) is
//!   Linux-only and is the **single** module containing `unsafe`; every test
//!   uses the programmable [`sys::mock::MockSys`] and never opens or grabs a
//!   real device.
//! * [`crate::device`] — `/dev/input/event*` enumeration, capability/axis/
//!   slot probing, and the explainable candidate/rejection verdict. The
//!   per-fd probe ([`crate::device::probe_open_fd`]) is shared between
//!   enumeration and the runtime session open, and every required capability
//!   response is validated for completeness.
//! * [`crate::event`] — safe decoding of kernel `input_event` bytes in the
//!   **x86_64 Linux layout** (24-byte `struct input_event`, two 8-byte
//!   `timeval` fields; the only live Linux ABI implemented and verified —
//!   other Linux targets fail at compile time instead of misdecoding, M4
//!   review RR3) and the checked `timeval` (kernel monotonic domain) →
//!   [`touchpad_core::Monotonic`] conversion. evdev defaults to
//!   `INPUT_CLK_REAL`; timestamps are monotonic only because the runtime
//!   issues `EVIOCSCLOCKID(CLOCK_MONOTONIC)` on its session fd before grab
//!   and before any read.
//! * [`crate::grab::DeviceHandle`] — the RAII grab guard: explicit opt-in,
//!   release attempted at most once even when the ungrab ioctl fails,
//!   best-effort `Drop` fallback.
//! * [`crate::snapshot::EvdevSnapshotSource`] — the real `SYN_DROPPED`
//!   snapshot adapter implementing [`ResyncSource`] via `EVIOCGMTSLOTS` /
//!   `EVIOCGKEY`, with fail-closed response-completeness validation.
//! * [`crate::runtime::EvdevRuntime`] — the open → read/decode → controlled
//!   shutdown lifecycle (stop work → output/flush boundary → idempotent
//!   ungrab → close fd), with fail-open cleanup on every fatal error, a
//!   monotonic-clock setup step before grab, and a resync drain rule that
//!   never replays events predating an installed snapshot.
//!
//! ## Safety
//!
//! This crate is `unsafe`-free except for [`sys::ffi`], the minimal
//! FFI/ioctl adapter, whose every `unsafe` block documents its safety
//! invariants (valid fd, matching request/payload layout, bounded kernel
//! writes).
//!
//! ## Cleanup guarantees (M4)
//!
//! No userspace cleanup can be guaranteed under `SIGKILL`, a kernel crash,
//! or a hard power loss: the kernel releases an evdev grab when the owning
//! fd is closed by process exit, but the ordered `ungrab`/`close` sequence
//! this crate performs is only guaranteed on paths it can run. No
//! real-hardware behavior is claimed by this milestone.

#![warn(missing_docs)]

pub mod bridge;
pub mod codes;
pub mod decode;
pub mod device;
pub mod event;
pub mod grab;
pub mod keyboard;
pub mod rawevent;
pub mod recorder;
pub mod replay;
pub mod resync;
pub mod runtime;
pub mod signals;
pub mod sink;
pub mod snapshot;
pub mod sys;

pub use bridge::TakeoverBridge;
pub use codes::{
    axis_id_for_code, ABS_MT_ORIENTATION, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE,
    ABS_MT_SLOT, ABS_MT_TOUCH_MAJOR, ABS_MT_TOUCH_MINOR, ABS_MT_TRACKING_ID, BTN_LEFT, BTN_MIDDLE,
    BTN_RIGHT, EV_ABS, EV_KEY, EV_SYN, INPUT_PROP_BUTTONPAD, INPUT_PROP_DIRECT, INPUT_PROP_POINTER,
    SYN_DROPPED, SYN_REPORT,
};
pub use decode::{DecodeError, SyncState, TypeBDecoder, MAX_SLOT_COUNT};
pub use device::{enumerate, pick_candidate, probe, ProbeError, ProbeReport, ProbeVerdict};
pub use event::{
    decode_input_events, encode_input_event, EventDecodeError, KernelEvent, TimevalError,
    INPUT_EVENT_SIZE,
};
pub use grab::{DeviceHandle, GrabError};
pub use keyboard::{discover_keyboards, KeyboardCandidate, KeyboardError, KeyboardMonitor};
pub use rawevent::RawEvent;
pub use recorder::{RawEventRecorder, RecorderError, TraceRecorder};
pub use replay::ReplayDecodeError;
pub use resync::{KernelStateSnapshot, ResyncSource, SlotSnapshot};
pub use runtime::{EvdevRuntime, OpenError, RuntimeError, RuntimePhase, ShutdownReport};
pub use signals::{
    install_termination_handler, termination_requested, SignalError, TerminationHandlerGuard,
};
pub use sink::{FrameSink, RecordingFrameSink};
pub use snapshot::{EvdevSnapshotSource, SnapshotError};
pub use sys::{AbsInfo, Fd, InputId, Sys, SysError, CLOCK_MONOTONIC};
