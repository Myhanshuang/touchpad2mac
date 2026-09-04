//! Windows platform boundary for touchpad2mac.
//!
//! This crate deliberately separates what Windows can do safely in ordinary
//! user mode from what requires a kernel filter driver:
//!
//! * Windows Precision Touchpads are discoverable as HID digitizer/touchpad
//!   top-level collections (usage page `0x0d`, usage `0x05`).
//! * Raw Input can observe HID data in the background, but Windows exposes no
//!   user-mode equivalent of Linux `EVIOCGRAB` for a Precision Touchpad.
//! * `SendInput` provides a compatibility output path for relative pointer,
//!   buttons, and wheel data.
//! * Recent Windows 11 builds expose native synthetic Precision Touchpad APIs
//!   (`CreateSyntheticPointerDevice2` / `InjectTouchpadAction`); availability
//!   is probed dynamically instead of assumed from the build SDK.
//!
//! Therefore this crate currently qualifies **overlay/probe** operation only.
//! A true physical-device takeover must not be advertised until a signed HID
//! or mouse-class filter driver owns the suppression boundary.

#![warn(missing_docs)]

mod capture;
mod device;
mod error;
mod output;
#[cfg(test)] // pure/tested groundwork; live descriptor wiring follows raw-hardware capture
mod overlay;
#[cfg(test)] // pure/tested groundwork; live descriptor wiring follows raw-hardware capture
mod ptp;
mod support;
#[cfg(target_os = "windows")]
mod win32;

pub use capture::{
    capture_precision_touchpad_raw_input, WindowsCaptureSummary, WindowsRawHidReport,
};
pub use device::WindowsTouchpadDevice;
pub use error::WindowsError;
pub use output::{
    emit_fixed_probe_pattern, EmitProbeOutcome, RealWindowsOutput, WindowsOutputApi,
    WindowsOutputSink,
};
pub use support::{probe_windows_support, render_windows_support, WindowsSupportReport};

/// Windows Precision Touchpad HID usage page (`Digitizers`).
pub const PRECISION_TOUCHPAD_USAGE_PAGE: u16 = 0x0d;
/// Windows Precision Touchpad HID top-level usage (`Touch Pad`).
pub const PRECISION_TOUCHPAD_USAGE: u16 = 0x05;

/// Enumerates Precision Touchpad top-level collections visible to the Win32
/// Raw Input device list.
pub fn enumerate_touchpads() -> Result<Vec<WindowsTouchpadDevice>, WindowsError> {
    #[cfg(target_os = "windows")]
    {
        win32::enumerate_touchpads()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(WindowsError::NotWindows)
    }
}
