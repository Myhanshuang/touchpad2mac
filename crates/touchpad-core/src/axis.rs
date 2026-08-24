//! Axis description and raw-to-millimeter conversion.
//!
//! Coordinate normalization must use the device-reported resolution
//! (`input_absinfo.resolution`). When a device reports none, the only
//! allowed path is an explicit [`crate::profile::DeviceProfile`] override;
//! conversion returns [`AxisConversionError::MissingResolution`] rather than
//! pretending to produce precise millimeters.
//!
//! ## Coordinate origin
//!
//! Absolute **position** conversion maps the axis minimum to `0 mm`:
//! `(raw - min) / resolution`. The origin (`AxisInfo::min`) is part of the
//! conversion, so a profile resolution override keeps the same origin
//! semantics — the override only replaces the resolution, never the origin.
//! The intermediate subtraction is done in `i64` so it cannot overflow `i32`.
//!
//! Relative **delta** conversion ([`raw_axis_delta_to_mm`]) has no origin: a
//! delta is `raw_delta / resolution`. The two conversions are intentionally
//! separate APIs because conflating them (e.g. forgetting to subtract `min`)
//! silently shifts absolute coordinates while leaving deltas correct.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::units::{Millimeters, RawAxis};

/// Description of a single device axis (range, filtering, resolution).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisInfo {
    /// Minimum raw value the axis reports.
    pub min: i32,
    /// Maximum raw value the axis reports.
    pub max: i32,
    /// Kernel `input_absinfo.fuzz` (value noise threshold).
    pub fuzz: i32,
    /// Kernel `input_absinfo.flat` (dead zone around center).
    pub flat: i32,
    /// Physical resolution in units per millimeter.
    ///
    /// `None` when the device does not report one (kernel
    /// `input_absinfo.resolution == 0`). Millimeter conversion then requires
    /// an explicit [`crate::profile::DeviceProfile`] override.
    pub resolution: Option<NonZeroU32>,
}

impl AxisInfo {
    /// Creates an axis description.
    #[must_use]
    pub fn new(min: i32, max: i32, fuzz: i32, flat: i32, resolution: Option<NonZeroU32>) -> Self {
        Self {
            min,
            max,
            fuzz,
            flat,
            resolution,
        }
    }

    /// Whether the axis description is structurally valid (`min <= max` and
    /// non-negative `fuzz`/`flat`, as the kernel reports unsigned values).
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.min <= self.max && self.fuzz >= 0 && self.flat >= 0
    }
}

/// Failure modes of raw → millimeter conversion.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AxisConversionError {
    /// The axis reports no resolution and no explicit profile override was
    /// supplied. Callers must either provide an override or keep the value
    /// in its unnormalized state / report a diagnostic.
    #[error("axis has no reported resolution; an explicit DeviceProfile override is required")]
    MissingResolution,
    /// The converted value is not finite (defensive; should not happen for
    /// i32/u32 inputs).
    #[error("raw-to-mm conversion produced a non-finite value")]
    NonFinite,
}

/// Converts a raw **absolute position** to physical millimeters using the
/// axis's reported resolution.
///
/// The axis minimum maps to `0 mm` (`(raw - min) / resolution`), computed in
/// `i64`/`f64` so the intermediate subtraction cannot overflow `i32`.
///
/// Fails with [`AxisConversionError::MissingResolution`] when the axis
/// reports no resolution.
pub fn raw_axis_position_to_mm(
    raw: RawAxis,
    info: &AxisInfo,
) -> Result<Millimeters, AxisConversionError> {
    let resolution = info
        .resolution
        .ok_or(AxisConversionError::MissingResolution)?;
    raw_axis_position_to_mm_with_resolution(raw, info, resolution)
}

/// Converts a raw **absolute position** to physical millimeters using an
/// explicit resolution (units per millimeter).
///
/// This is the only allowed path when the device reports no resolution; the
/// resolution must come from an explicit [`crate::profile::DeviceProfile`]
/// override, never from guessing. Origin semantics are identical to
/// [`raw_axis_position_to_mm`]: the axis minimum still maps to `0 mm`; only
/// the resolution is replaced.
pub fn raw_axis_position_to_mm_with_resolution(
    raw: RawAxis,
    info: &AxisInfo,
    resolution: NonZeroU32,
) -> Result<Millimeters, AxisConversionError> {
    let offset = i64::from(raw.as_i32()) - i64::from(info.min);
    let mm = offset as f64 / f64::from(resolution.get());
    Millimeters::try_new(mm as f32).map_err(|_| AxisConversionError::NonFinite)
}

/// Converts a raw **relative delta** to physical millimeters.
///
/// Deltas carry no axis origin: the conversion is `raw_delta / resolution`.
/// The resolution must come from the device report or an explicit
/// [`crate::profile::DeviceProfile`] override; `None` yields
/// [`AxisConversionError::MissingResolution`].
pub fn raw_axis_delta_to_mm(
    raw_delta: RawAxis,
    resolution: Option<NonZeroU32>,
) -> Result<Millimeters, AxisConversionError> {
    let resolution = resolution.ok_or(AxisConversionError::MissingResolution)?;
    let mm = f64::from(raw_delta.as_i32()) / f64::from(resolution.get());
    Millimeters::try_new(mm as f32).map_err(|_| AxisConversionError::NonFinite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::RawAxis;

    fn axis_with(resolution: Option<NonZeroU32>) -> AxisInfo {
        AxisInfo::new(0, 1000, 0, 0, resolution)
    }

    fn mm(value: f32) -> Millimeters {
        Millimeters::try_new(value).unwrap()
    }

    #[test]
    fn position_converts_with_reported_resolution() {
        let info = axis_with(NonZeroU32::new(100));
        // min == 0: position in mm equals raw / resolution.
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(150), &info).unwrap(),
            mm(1.5)
        );
        // Below-origin raw value -> negative millimeters.
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(-150), &info).unwrap(),
            mm(-1.5)
        );
    }

    #[test]
    fn position_maps_axis_min_to_zero_mm() {
        // A real absolute axis with min != 0: origin must be honored.
        let info = AxisInfo::new(100, 500, 0, 0, NonZeroU32::new(100));
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(100), &info).unwrap(),
            mm(0.0)
        );
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(300), &info).unwrap(),
            mm(2.0)
        );
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(500), &info).unwrap(),
            mm(4.0)
        );
        // The same raw value on an axis whose min == 0 would differ; the
        // origin is part of the conversion, not an approximation.
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(300), &axis_with(NonZeroU32::new(100))).unwrap(),
            mm(3.0)
        );
    }

    #[test]
    fn position_boundary_values_do_not_overflow() {
        // (raw - min) can span the full i32 range; the i64 intermediate
        // keeps the subtraction exact instead of overflowing i32.
        let info = AxisInfo::new(i32::MIN, i32::MIN, 0, 0, NonZeroU32::new(1));
        let hi = raw_axis_position_to_mm(RawAxis::new(i32::MAX), &info).unwrap();
        assert!(hi.is_finite());
        assert!(hi.as_mm() > 0.0);

        let info = AxisInfo::new(i32::MAX, i32::MAX, 0, 0, NonZeroU32::new(1));
        let lo = raw_axis_position_to_mm(RawAxis::new(i32::MIN), &info).unwrap();
        assert!(lo.is_finite());
        assert!(lo.as_mm() < 0.0);

        // The offset spans 2^32 - 1 counts; the nearest f32 to ±4294967295
        // is ±4294967296 (±2^32), which is exactly representable.
        assert_eq!(hi.as_mm(), 4_294_967_296.0);
        assert_eq!(lo.as_mm(), -4_294_967_296.0);

        // min == max is a degenerate but valid axis: the only representable
        // position is 0 mm.
        let info = AxisInfo::new(42, 42, 0, 0, NonZeroU32::new(10));
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(42), &info).unwrap(),
            mm(0.0)
        );
    }

    #[test]
    fn profile_override_keeps_origin_semantics() {
        // The override replaces only the resolution; the axis min still maps
        // to 0 mm.
        let info = AxisInfo::new(100, 500, 0, 0, None);
        let override_resolution = NonZeroU32::new(50).unwrap();
        assert_eq!(
            raw_axis_position_to_mm_with_resolution(RawAxis::new(100), &info, override_resolution)
                .unwrap(),
            mm(0.0)
        );
        assert_eq!(
            raw_axis_position_to_mm_with_resolution(RawAxis::new(300), &info, override_resolution)
                .unwrap(),
            mm(4.0)
        );
        // With the device resolution it would be 2.0 mm; the override changes
        // the scale, not the origin.
        assert_eq!(
            raw_axis_position_to_mm_with_resolution(
                RawAxis::new(300),
                &info,
                NonZeroU32::new(100).unwrap()
            )
            .unwrap(),
            mm(2.0)
        );
    }

    #[test]
    fn missing_resolution_is_an_error() {
        let info = axis_with(None);
        assert_eq!(
            raw_axis_position_to_mm(RawAxis::new(150), &info),
            Err(AxisConversionError::MissingResolution)
        );
        assert_eq!(
            raw_axis_delta_to_mm(RawAxis::new(150), None),
            Err(AxisConversionError::MissingResolution)
        );
    }

    #[test]
    fn delta_conversion_has_no_origin() {
        // Deltas are scale-only: the same delta converts identically no
        // matter the axis origin.
        let resolution = NonZeroU32::new(100);
        assert_eq!(
            raw_axis_delta_to_mm(RawAxis::new(150), resolution).unwrap(),
            mm(1.5)
        );
        assert_eq!(
            raw_axis_delta_to_mm(RawAxis::new(-150), resolution).unwrap(),
            mm(-1.5)
        );
        let origin_at_100 = AxisInfo::new(100, 500, 0, 0, NonZeroU32::new(100));
        let origin_at_0 = AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100));
        let delta = RawAxis::new(150);
        assert_eq!(
            raw_axis_delta_to_mm(delta, origin_at_100.resolution).unwrap(),
            raw_axis_delta_to_mm(delta, origin_at_0.resolution).unwrap()
        );
    }

    #[test]
    fn explicit_override_is_the_other_allowed_path_for_position() {
        let info = axis_with(None);
        let resolution = NonZeroU32::new(50).unwrap();
        assert_eq!(
            raw_axis_position_to_mm_with_resolution(RawAxis::new(100), &info, resolution).unwrap(),
            mm(2.0)
        );
    }

    #[test]
    fn axis_info_range_validation() {
        assert!(axis_with(None).is_valid());
        assert!(!AxisInfo::new(100, 0, 0, 0, None).is_valid());
        assert!(!AxisInfo::new(0, 100, -1, 0, None).is_valid());
    }
}
