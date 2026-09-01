//! Platform-neutral description of a Windows Precision Touchpad device.

#![forbid(unsafe_code)]

/// One Windows Raw Input HID device whose top-level collection identifies it
/// as a Precision Touchpad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsTouchpadDevice {
    /// Win32 raw-input device path, suitable for diagnostics and stable
    /// selection during one boot/session.
    pub device_name: String,
    /// HID vendor id reported by `RID_DEVICE_INFO_HID`.
    pub vendor_id: u32,
    /// HID product id reported by `RID_DEVICE_INFO_HID`.
    pub product_id: u32,
    /// HID device version reported by `RID_DEVICE_INFO_HID`.
    pub version_number: u32,
    /// HID top-level usage page. Precision Touchpads use `0x0d`.
    pub usage_page: u16,
    /// HID top-level usage. Precision Touchpads use `0x05`.
    pub usage: u16,
}

impl WindowsTouchpadDevice {
    /// Returns the conventional `VID_xxxx&PID_xxxx` identity fragment.
    #[must_use]
    pub fn vid_pid(&self) -> String {
        format!("VID_{:04X}&PID_{:04X}", self.vendor_id, self.product_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vid_pid_is_stable_and_uppercase() {
        let device = WindowsTouchpadDevice {
            device_name: r"\\?\HID#TEST".to_string(),
            vendor_id: 0x06cb,
            product_id: 0xce2d,
            version_number: 1,
            usage_page: 0x0d,
            usage: 0x05,
        };
        assert_eq!(device.vid_pid(), "VID_06CB&PID_CE2D");
    }
}
