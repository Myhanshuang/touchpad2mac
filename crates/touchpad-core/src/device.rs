//! Platform-agnostic device descriptor.
//!
//! The platform input layer (Linux evdev, future Windows/macOS backends)
//! translates its device enumeration into a [`DeviceDescriptor`] so the core
//! never depends on platform-specific device APIs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::axis::AxisInfo;
use crate::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLevel};
use crate::profile::DeviceProfile;

/// Opaque identifier for a device axis, assigned by the platform input
/// layer. The Linux layer maps kernel `ABS_*` codes onto these ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AxisId(u32);

impl AxisId {
    /// Creates an axis id.
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// The raw id value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// A device as discovered by the platform input layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    /// Device name as reported by the platform (e.g. kernel device name).
    pub name: String,
    /// USB vendor id (0 when unknown).
    pub vendor_id: u16,
    /// USB product id (0 when unknown).
    pub product_id: u16,
    /// Axis descriptions keyed by [`AxisId`].
    pub axes: BTreeMap<AxisId, AxisInfo>,
    /// Number of Type-B slots, when the device exposes MT slots.
    pub slot_count: Option<u32>,
    /// Whether the device reports multitouch via the Type-B slot protocol.
    pub supports_type_b_mt: bool,
    /// Whether the device has dedicated physical button events.
    pub has_physical_buttons: bool,
    /// Explicit hardware adjustments (resolution overrides, quirks).
    pub profile: DeviceProfile,
}

impl DeviceDescriptor {
    /// Creates a descriptor with no axes, no slots, and an empty profile.
    /// Fields are public so the platform layer fills in what it discovers.
    #[must_use]
    pub fn new(name: impl Into<String>, vendor_id: u16, product_id: u16) -> Self {
        Self {
            name: name.into(),
            vendor_id,
            product_id,
            axes: BTreeMap::new(),
            slot_count: None,
            supports_type_b_mt: false,
            has_physical_buttons: false,
            profile: DeviceProfile::new("default"),
        }
    }

    /// Structural validation; returns diagnostics instead of panicking.
    ///
    /// Currently checks axis range validity. Later milestones (device
    /// probing, slot handling) extend this with consistency checks against
    /// real hardware reports.
    #[must_use]
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for (axis, info) in &self.axes {
            if !info.is_valid() {
                out.push(Diagnostic::new(
                    DiagnosticLevel::Error,
                    DiagnosticCode::InvalidAxisRange,
                    format!(
                        "axis {:?} has invalid range min={} max={} (fuzz={}, flat={})",
                        axis, info.min, info.max, info.fuzz, info.flat
                    ),
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> DeviceDescriptor {
        let mut descriptor = DeviceDescriptor::new("test touchpad", 0x1234, 0x5678);
        descriptor.slot_count = Some(10);
        descriptor.supports_type_b_mt = true;
        descriptor.has_physical_buttons = true;
        descriptor
    }

    #[test]
    fn valid_descriptor_has_no_diagnostics() {
        assert!(descriptor().validate().is_empty());
    }

    #[test]
    fn invalid_axis_range_is_diagnosed() {
        let mut descriptor = descriptor();
        descriptor
            .axes
            .insert(AxisId::new(1), AxisInfo::new(100, 0, 0, 0, None));
        let diags = descriptor.validate();
        assert!(diags
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidAxisRange));
    }

    #[test]
    fn axis_ids_are_distinct() {
        assert_ne!(AxisId::new(1), AxisId::new(2));
        assert_eq!(AxisId::new(1), AxisId::new(1));
    }
}
