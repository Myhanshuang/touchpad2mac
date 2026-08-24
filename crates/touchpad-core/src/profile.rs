//! Device profiles: explicit per-device adjustments (design.md §6
//! `DeviceProfile` / `DeviceQuirk`).
//!
//! A profile never invents data: it only supplies values the device fails to
//! report (e.g. axis resolution) and records known hardware idiosyncrasies.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::axis::AxisInfo;
use crate::device::AxisId;

/// Known hardware idiosyncrasies that change how input is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceQuirk {
    /// The device reports no (or unreliable) axis resolution; millimeter
    /// conversion for affected axes requires an explicit profile override.
    UnreliableResolution,
    /// The device is a unified buttonpad: physical clicks arrive through
    /// touch contacts, not dedicated button events.
    Buttonpad,
}

/// Explicit per-device adjustments.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfile {
    /// Human-readable profile name.
    pub name: String,
    /// Explicit resolutions (units per millimeter) for axes that report
    /// none. Keyed by opaque [`AxisId`] assigned by the platform layer.
    axis_resolutions: BTreeMap<AxisId, NonZeroU32>,
    /// Known quirks of the device.
    pub quirks: Vec<DeviceQuirk>,
}

impl DeviceProfile {
    /// Creates an empty profile.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            axis_resolutions: BTreeMap::new(),
            quirks: Vec::new(),
        }
    }

    /// Known-hardware profile selection. Unknown devices get the generic
    /// profile. Entries encode only observed/structural facts and never
    /// fabricate missing sensor capabilities.
    #[must_use]
    pub fn for_hardware(vendor_id: u16, product_id: u16) -> Self {
        match (vendor_id, product_id) {
            // CIRQ1080:00 0488:1054 Touchpad observed during M5 bring-up.
            (0x0488, 0x1054) => Self::new("cirq1080-0488-1054").with_quirk(DeviceQuirk::Buttonpad),
            _ => Self::new("default"),
        }
    }

    /// The explicit resolution override for an axis, if any.
    #[must_use]
    pub fn resolution_override(&self, axis: AxisId) -> Option<NonZeroU32> {
        self.axis_resolutions.get(&axis).copied()
    }

    /// Builder: adds an explicit resolution override for an axis.
    #[must_use]
    pub fn with_axis_resolution(mut self, axis: AxisId, resolution: NonZeroU32) -> Self {
        self.axis_resolutions.insert(axis, resolution);
        self
    }

    /// Builder: adds a quirk (deduplicated).
    #[must_use]
    pub fn with_quirk(mut self, quirk: DeviceQuirk) -> Self {
        if !self.quirks.contains(&quirk) {
            self.quirks.push(quirk);
        }
        self
    }

    /// Resolution to use for an axis: the explicit profile override wins;
    /// otherwise the device-reported resolution. `None` when neither is
    /// available, in which case the value must stay unnormalized (or a
    /// diagnostic must be produced).
    #[must_use]
    pub fn effective_resolution(&self, axis: AxisId, info: &AxisInfo) -> Option<NonZeroU32> {
        self.resolution_override(axis).or(info.resolution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_resolution_prefers_override() {
        let axis = AxisId::new(0);
        let info = AxisInfo::new(0, 100, 0, 0, NonZeroU32::new(10));
        let profile =
            DeviceProfile::new("test").with_axis_resolution(axis, NonZeroU32::new(100).unwrap());
        assert_eq!(
            profile.effective_resolution(axis, &info),
            NonZeroU32::new(100)
        );

        // A different axis falls back to the device-reported resolution.
        let other = AxisId::new(1);
        assert_eq!(
            profile.effective_resolution(other, &info),
            NonZeroU32::new(10)
        );
    }

    #[test]
    fn no_resolution_anywhere_is_none() {
        let axis = AxisId::new(0);
        let info = AxisInfo::new(0, 100, 0, 0, None);
        let profile = DeviceProfile::new("test");
        assert_eq!(profile.effective_resolution(axis, &info), None);
        assert_eq!(profile.resolution_override(axis), None);
    }

    #[test]
    fn quirks_dedup() {
        let profile = DeviceProfile::new("test")
            .with_quirk(DeviceQuirk::Buttonpad)
            .with_quirk(DeviceQuirk::Buttonpad);
        assert_eq!(profile.quirks, vec![DeviceQuirk::Buttonpad]);
    }

    #[test]
    fn cirq1080_profile_contains_only_the_observed_buttonpad_quirk() {
        let profile = DeviceProfile::for_hardware(0x0488, 0x1054);
        assert_eq!(profile.name, "cirq1080-0488-1054");
        assert_eq!(profile.quirks, vec![DeviceQuirk::Buttonpad]);
        assert!(profile.axis_resolutions.is_empty());
        assert_eq!(DeviceProfile::for_hardware(1, 2).name, "default");
    }
}
