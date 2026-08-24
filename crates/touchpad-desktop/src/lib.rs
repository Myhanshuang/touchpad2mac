//! # touchpad-desktop — KDE Wayland output backend adapter (M6)
//!
//! A safe, testable slice of the desktop **output** path for the Touchpad
//! Runtime: it translates the typed [`touchpad_core::OutputSink`] contract
//! (relative pointer motion, primary/secondary buttons, pixel-precise smooth
//! scroll lifecycle) onto the XDG **RemoteDesktop portal** (D-Bus) + **libei**
//! sender stack available on this host, with an explicit lifecycle, honest
//! structured failures, and idempotent `release_all` on every path.
//!
//! ```text
//! OutputEvent (typed contract, touchpad-core)
//!         ↓
//! PortalOutputSink  — session lifecycle + held-state tracking + release_all
//!         ↓
//! Portal (zbus)  ── CreateSession/SelectDevices/Start/ConnectToEIS ──▶ EIS fd
//! Transport (libei sender) ── relative motion / buttons / pixel scroll ──▶ compositor
//! ```
//!
//! # ABI choice (documented, environment-based)
//!
//! * **Portal**: `org.freedesktop.portal.RemoteDesktop` **interface version
//!   2** (observed on this host by D-Bus introspection), whose
//!   `ConnectToEIS` method returns the EIS socket fd; `SelectDevices`
//!   requests the pointer device type.
//! * **libei**: soname `libei.so.1` (1.x; 1.6.0 installed), **loaded at
//!   run time** via `libloading` so a missing library is an honest runtime
//!   result and the workspace builds/tests without the library.
//! * **D-Bus**: pure-Rust `zbus` blocking API — no system D-Bus library is
//!   linked.
//!
//! # Safety (M6)
//!
//! * This crate never opens, reads, records, or grabs any physical
//!   `/dev/input` device, and never creates a virtual touchpad or exposes
//!   raw contacts/finger counts (touch capability is deliberately not
//!   bound).
//! * The only `unsafe` is the minimal libei FFI boundary (the **crate-private**
//!   `ffi` module, Linux-only, runtime-loaded) with documented safety
//!   invariants; its handle types are non-`Copy` RAII owners released
//!   exactly once, so safe code cannot duplicate, double-release, or use
//!   owned libei references illegally (M6 re-review R1). Every other module
//!   is `#![forbid(unsafe_code)]`.
//! * No test emits real desktop input: tests drive
//!   [`fake::FakePortal`]/[`fake::FakeTransport`] only; the real zbus portal
//!   and libei transport are never constructed in tests.
//! * Output preparation and authorization ([`sink::PortalOutputSink::prepare`])
//!   are designed to complete before any future `EVIOCGRAB` (M10);
//!   M6 itself never grabs.
//!
//! # Honest status
//!
//! The backend is **`experimental/unqualified`** until a reviewer actually
//! runs and measures `touchpadctl output-probe --emit` on the current KDE
//! Wayland session (relative-delta displacement A/B, pixel scroll, button
//! release; see `docs/M6_ACCEPTANCE.md`).

#![warn(missing_docs)]

pub mod capabilities;
pub mod desktop;
pub mod emit;
pub mod error;
pub mod fake;
pub(crate) mod ffi;
pub mod held;
pub mod kde_actions;
#[cfg(target_os = "linux")]
pub(crate) mod native_transport;
pub mod portal;
pub mod portal_zbus;
pub mod probe;
pub mod sink;
pub mod streaming;
pub mod transport;

pub use capabilities::{Capability, OutputCapabilities};
#[cfg(target_os = "linux")]
pub use desktop::PortalDesktopOutput;
pub use desktop::{DesktopOutput, EmitDriver, UnsupportedDesktopOutput};
pub use emit::{pattern, run_pattern, EmitOutcome, PatternStep, MAX_PATTERN_EVENTS};
pub use error::DesktopOutputError;
pub use fake::{
    FakeDesktopOutput, FakePortal, FakeStreamingOutput, FakeStreamingOutputFactory,
    FakeStreamingState, FakeTransport, FakeWireCall,
};
pub use held::HeldState;
pub use kde_actions::{
    real_kde_action_supported, required_real_kde_actions, KGlobalAccelTransport, KdeActionAdapter,
    KdeActionBinding, KdeActionMap, KdeActionTransport,
};
pub use portal::{EisFd, Portal, PortalSession};
pub use probe::{ProbeReport, ProbeSource};
pub use sink::{PortalOutputSink, SessionState};
pub use streaming::{
    KdeActionStreamingOutput, PortalStreamingOutput, RealKdeStreamingOutputFactory,
    RealStreamingOutputFactory, StreamingOutput, StreamingOutputFactory,
};
pub use transport::{DeviceId, SeatId, Transport, TransportEvent};
