//! Honest Windows capability report.

#![forbid(unsafe_code)]

#[cfg(any(test, target_os = "windows"))]
use crate::WindowsError;
use crate::WindowsTouchpadDevice;

/// Read-only Windows backend capability report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsSupportReport {
    /// Build/runtime target reported by Rust.
    pub target_os: &'static str,
    /// Precision Touchpad Raw Input devices visible to the process.
    pub touchpads: Vec<WindowsTouchpadDevice>,
    /// Whether the background Raw Input API is usable on this platform.
    /// This is an observation about the API surface, not a claim that raw HID
    /// decoding or suppression has been qualified on the current machine.
    pub background_raw_input: bool,
    /// Whether the compatibility `SendInput` output path is available.
    pub send_input_output: bool,
    /// Whether all native synthetic-touchpad entry points required by the
    /// Windows 11 PT_TOUCHPAD path are exported by the running `user32.dll`.
    pub native_synthetic_touchpad: bool,
    /// Whether a pure user-mode full takeover can suppress the physical PTP
    /// while still consuming its raw contacts. This is intentionally false:
    /// Raw Input has no touchpad equivalent of Linux `EVIOCGRAB`.
    pub user_mode_full_takeover: bool,
    /// Human-readable blocker for full takeover.
    pub takeover_blocker: String,
}

#[cfg(any(test, target_os = "windows"))]
trait SupportApi {
    fn enumerate_touchpads(&self) -> Result<Vec<WindowsTouchpadDevice>, WindowsError>;
    fn send_input_available(&self) -> bool;
    fn background_raw_input_available(&self) -> bool;
    fn native_synthetic_touchpad_available(&self) -> bool;
}

/// Probes the current host without emitting input or opening a live capture
/// session.
#[must_use]
pub fn probe_windows_support() -> WindowsSupportReport {
    #[cfg(target_os = "windows")]
    {
        probe_with(&crate::win32::Win32SupportApi)
    }
    #[cfg(not(target_os = "windows"))]
    {
        WindowsSupportReport {
            target_os: std::env::consts::OS,
            touchpads: Vec::new(),
            background_raw_input: false,
            send_input_output: false,
            native_synthetic_touchpad: false,
            user_mode_full_takeover: false,
            takeover_blocker:
                "the Windows backend can only run on Windows; full takeover also requires a signed input filter driver"
                    .to_string(),
        }
    }
}

#[cfg(any(test, target_os = "windows"))]
fn probe_with(api: &dyn SupportApi) -> WindowsSupportReport {
    let touchpads = api.enumerate_touchpads().unwrap_or_default();
    WindowsSupportReport {
        target_os: std::env::consts::OS,
        touchpads,
        background_raw_input: api.background_raw_input_available(),
        send_input_output: api.send_input_available(),
        native_synthetic_touchpad: api.native_synthetic_touchpad_available(),
        user_mode_full_takeover: false,
        takeover_blocker: "Windows Raw Input can observe a Precision Touchpad in the background, but RIDEV_NOLEGACY suppresses only mouse/keyboard legacy messages; a signed HID/mouse-class filter driver is required to suppress the physical touchpad without double input".to_string(),
    }
}

/// Renders a stable diagnostic report for `touchpadctl windows-probe`.
#[must_use]
pub fn render_windows_support(report: &WindowsSupportReport) -> String {
    let mut lines = vec![
        "Windows backend state: user-mode overlay/probe implemented; full takeover gated"
            .to_string(),
        format!("target OS: {}", report.target_os),
        format!(
            "background Raw Input: {}",
            yes_no(report.background_raw_input)
        ),
        format!(
            "SendInput semantic output: {}",
            yes_no(report.send_input_output)
        ),
        format!(
            "native synthetic Precision Touchpad API: {}",
            yes_no(report.native_synthetic_touchpad)
        ),
        format!(
            "pure user-mode full takeover: {}",
            yes_no(report.user_mode_full_takeover)
        ),
    ];
    if report.touchpads.is_empty() {
        lines.push(
            "Precision Touchpads: none discovered by Raw Input device enumeration".to_string(),
        );
    } else {
        lines.push(format!(
            "Precision Touchpads: {} discovered",
            report.touchpads.len()
        ));
        for device in &report.touchpads {
            lines.push(format!(
                "  - {} {} usage={:#04x}/{:#04x} path={}",
                device.vid_pid(),
                device.version_number,
                device.usage_page,
                device.usage,
                device.device_name
            ));
        }
    }
    lines.push(format!("takeover blocker: {}", report.takeover_blocker));
    lines.join("\n")
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "available"
    } else {
        "unavailable"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeApi;

    impl SupportApi for FakeApi {
        fn enumerate_touchpads(&self) -> Result<Vec<WindowsTouchpadDevice>, WindowsError> {
            Ok(vec![WindowsTouchpadDevice {
                device_name: "ptp0".into(),
                vendor_id: 0x1234,
                product_id: 0x5678,
                version_number: 2,
                usage_page: 0x0d,
                usage: 0x05,
            }])
        }

        fn send_input_available(&self) -> bool {
            true
        }

        fn background_raw_input_available(&self) -> bool {
            true
        }

        fn native_synthetic_touchpad_available(&self) -> bool {
            true
        }
    }

    #[test]
    fn report_never_claims_user_mode_takeover() {
        let report = probe_with(&FakeApi);
        assert_eq!(report.touchpads.len(), 1);
        assert!(report.background_raw_input);
        assert!(report.native_synthetic_touchpad);
        assert!(!report.user_mode_full_takeover);
        assert!(report.takeover_blocker.contains("filter driver"));
    }

    #[test]
    fn rendered_report_names_the_ptp_identity_and_blocker() {
        let text = render_windows_support(&probe_with(&FakeApi));
        assert!(text.contains("VID_1234&PID_5678"), "{text}");
        assert!(text.contains("full takeover"), "{text}");
        assert!(text.contains("filter driver"), "{text}");
    }
}

#[cfg(target_os = "windows")]
impl SupportApi for crate::win32::Win32SupportApi {
    fn enumerate_touchpads(&self) -> Result<Vec<WindowsTouchpadDevice>, WindowsError> {
        crate::win32::enumerate_touchpads()
    }

    fn send_input_available(&self) -> bool {
        true
    }

    fn background_raw_input_available(&self) -> bool {
        true
    }

    fn native_synthetic_touchpad_available(&self) -> bool {
        crate::win32::synthetic_touchpad_exports_available()
    }
}
