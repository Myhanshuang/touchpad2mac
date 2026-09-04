//! Bounded Windows Precision Touchpad Raw Input capture.
//!
//! This is deliberately a bring-up/diagnostic primitive. It does not suppress
//! Windows' native Precision Touchpad stack and it does not inject output.
//! The capture path is useful on older Windows 10 machines because it relies
//! only on the long-established Raw Input API rather than the newer Windows
//! 11 synthetic-touchpad APIs.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::WindowsError;

/// One HID input report delivered by the Precision Touchpad Raw Input TLC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsRawHidReport {
    /// Opaque Raw Input device handle rendered as an integer. The value is
    /// diagnostic-only and is not stable across boots or reconnects.
    pub device_handle: usize,
    /// Zero-based report index inside a batched `RAWHID` payload.
    pub batch_index: u32,
    /// Raw HID input report bytes, including the report ID byte when the
    /// descriptor uses report IDs.
    pub bytes: Vec<u8>,
}

/// Summary returned by a bounded Windows Raw Input capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowsCaptureSummary {
    /// Number of `WM_INPUT` messages accepted for HID input.
    pub raw_input_messages: u64,
    /// Number of individual HID reports delivered across those messages.
    pub hid_reports: u64,
    /// Number of HID payload bytes delivered across all reports.
    pub hid_bytes: u64,
}

/// Captures Precision Touchpad Raw Input reports for at most `duration`.
///
/// The callback runs synchronously on the capture thread. The function does
/// not suppress native Windows touchpad handling and never emits synthetic
/// input, making it suitable as the first hardware-qualification step on an
/// unfamiliar or older Windows host.
pub fn capture_precision_touchpad_raw_input<F>(
    duration: Duration,
    mut on_report: F,
) -> Result<WindowsCaptureSummary, WindowsError>
where
    F: FnMut(WindowsRawHidReport) -> Result<(), WindowsError>,
{
    if duration.is_zero() {
        return Err(WindowsError::Unsupported(
            "a non-zero Windows capture duration".to_string(),
        ));
    }
    #[cfg(target_os = "windows")]
    {
        crate::win32::capture_precision_touchpad_raw_input(duration, &mut on_report)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &mut on_report;
        Err(WindowsError::NotWindows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_duration_is_rejected_before_platform_access() {
        let result = capture_precision_touchpad_raw_input(Duration::ZERO, |_| Ok(()));
        assert!(matches!(result, Err(WindowsError::Unsupported(_))));
    }
}
