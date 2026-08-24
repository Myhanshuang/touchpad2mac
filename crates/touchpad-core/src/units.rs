//! Typed physical and logical units.
//!
//! The types in this module exist to make unit confusion a compile error:
//! a raw device count, a millimeter, and a logical pixel are all `f32`/`i32`
//! under the hood but are not interchangeable without an explicit conversion.
//!
//! Invariant: [`Millimeters`] and [`LogicalPixels`] never carry `NaN` or
//! infinity. Every public constructor and every `Deserialize`
//! implementation validates finiteness, and the inner field is private, so
//! the invariant cannot be bypassed from outside this crate.
//!
//! Fail-open policy: the runtime may exclusively grab a physical touchpad
//! (design.md §14), so a panic must never be the error-handling strategy for
//! runtime input or arithmetic. This module therefore exposes **no panicking
//! constructors and no arithmetic operator traits** (operator impls used to
//! funnel through a panicking `new`). Values are built with the structured
//! fallible [`Millimeters::try_new`] / [`LogicalPixels::try_new`], and
//! arithmetic goes through the checked operations, which report overflow via
//! `Option` instead of panicking.

use serde::{Deserialize, Deserializer, Serialize};

use crate::validation::deserialize_finite_f32;

/// Structured error returned when a value that must be finite is not.
///
/// `NaN` and infinities are rejected by every construction path so they can
/// never enter a [`Millimeters`] or [`LogicalPixels`] value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("value must be finite (no NaN or infinity)")]
pub struct NonFiniteError;

/// Structured error returned when a millimetre-to-pixel scale is invalid.
///
/// A pointer mapping scale must be finite **and strictly positive**: zero or
/// negative scales would invert or zero the mapping, and `NaN`/infinities are
/// never permitted in core units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("scale must be finite and strictly positive (no NaN, infinity, zero, or negative values)")]
pub struct ScaleError;

/// A physical distance in millimeters.
///
/// Produced exclusively through [`crate::axis::raw_axis_position_to_mm`],
/// [`crate::axis::raw_axis_delta_to_mm`], or a
/// [`crate::profile::DeviceProfile`] resolution override — never by
/// directly reinterpreting a raw axis value.
///
/// Values may be negative (e.g. relative deltas) but are always finite.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize)]
pub struct Millimeters(f32);

impl Millimeters {
    /// Zero millimeters.
    pub const ZERO: Millimeters = Millimeters(0.0);

    /// Creates a value from a finite number of millimeters.
    ///
    /// Returns [`NonFiniteError`] when `mm` is `NaN` or infinite; this is
    /// the only public constructor — there is deliberately no panicking
    /// variant.
    pub fn try_new(mm: f32) -> Result<Self, NonFiniteError> {
        if mm.is_finite() {
            Ok(Self(mm))
        } else {
            Err(NonFiniteError)
        }
    }

    /// The value in millimeters.
    #[must_use]
    pub const fn as_mm(self) -> f32 {
        self.0
    }

    /// Whether the value is finite. Always true for values built through the
    /// public API; useful as a defense-in-depth check.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    /// Checked addition; `None` when the result would be non-finite.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0 + rhs.0).ok()
    }

    /// Checked subtraction; `None` when the result would be non-finite.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0 - rhs.0).ok()
    }
}

impl std::fmt::Display for Millimeters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}mm", self.0)
    }
}

impl<'de> Deserialize<'de> for Millimeters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_finite_f32(deserializer).map(Self)
    }
}

/// A raw axis value in device counts, as reported by the input device.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RawAxis(i32);

impl RawAxis {
    /// Creates a raw axis value from a device count.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// The raw device count.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self.0
    }
}

/// A logical pointer/scroll delta in device-independent pixels.
///
/// This is the unit of *semantic output*; it must not be confused with the
/// raw axis units a physical device reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize)]
pub struct LogicalPixels(f32);

impl LogicalPixels {
    /// Zero pixels.
    pub const ZERO: LogicalPixels = LogicalPixels(0.0);

    /// Creates a value from a finite number of logical pixels.
    ///
    /// Returns [`NonFiniteError`] when `px` is `NaN` or infinite; this is
    /// the only public constructor — there is deliberately no panicking
    /// variant.
    pub fn try_new(px: f32) -> Result<Self, NonFiniteError> {
        if px.is_finite() {
            Ok(Self(px))
        } else {
            Err(NonFiniteError)
        }
    }

    /// The value in logical pixels.
    #[must_use]
    pub const fn as_px(self) -> f32 {
        self.0
    }

    /// Whether the value is finite. Always true for values built through the
    /// public API; useful as a defense-in-depth check.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    /// Checked addition; `None` when the result would be non-finite.
    #[must_use]
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0 + rhs.0).ok()
    }

    /// Checked subtraction; `None` when the result would be non-finite.
    #[must_use]
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        Self::try_new(self.0 - rhs.0).ok()
    }
}

impl<'de> Deserialize<'de> for LogicalPixels {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_finite_f32(deserializer).map(Self)
    }
}

/// A linear scale mapping physical millimeters to logical pixels.
///
/// This is the explicitly configured M7 pointer mapping: the number of
/// logical pixels produced per physical millimeter of contact motion. It is a
/// scale, not an acceleration curve — M11 owns acceleration, jitter
/// filtering, and velocity-based curves (PHASE2_PLAN.md §5 M7/M11).
///
/// Invariant: the value is always finite and strictly positive. `NaN`,
/// infinities, zero, and negative values are rejected at construction; a
/// non-positive scale would zero or invert the pointer mapping, so it can
/// never enter a configuration.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct LogicalPixelsPerMm(f32);

impl LogicalPixelsPerMm {
    /// Creates a scale from a finite, strictly positive number of logical
    /// pixels per millimeter.
    ///
    /// Returns [`ScaleError`] when `px_per_mm` is `NaN`, infinite, zero, or
    /// negative; this is the only public constructor.
    pub fn try_new(px_per_mm: f32) -> Result<Self, ScaleError> {
        if px_per_mm.is_finite() && px_per_mm > 0.0 {
            Ok(Self(px_per_mm))
        } else {
            Err(ScaleError)
        }
    }

    /// The scale in logical pixels per millimeter.
    #[must_use]
    pub const fn as_px_per_mm(self) -> f32 {
        self.0
    }

    /// Whether the value is finite. Always true for values built through the
    /// public API; useful as a defense-in-depth check.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.0.is_finite()
    }
}

impl std::fmt::Display for LogicalPixelsPerMm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}px/mm", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{Error, F32Deserializer};

    type UnitDeserializer = F32Deserializer<Error>;

    #[test]
    fn units_are_distinct_newtypes() {
        // Compile-time check that these are different types: assigning a
        // `RawAxis` where a `Millimeters` is expected must not compile. We
        // can only assert the runtime representation here.
        let mm = Millimeters::try_new(1.5).unwrap();
        let raw = RawAxis::new(150);
        assert_eq!(mm.as_mm(), 1.5);
        assert_eq!(raw.as_i32(), 150);
        assert_eq!(mm, Millimeters::try_new(1.5).unwrap());
        assert_ne!(mm, Millimeters::try_new(2.0).unwrap());
    }

    #[test]
    fn logical_pixels_accumulate_via_checked_ops() {
        let mut p = LogicalPixels::ZERO;
        p = p.checked_add(LogicalPixels::try_new(1.0).unwrap()).unwrap();
        p = p.checked_add(LogicalPixels::try_new(2.5).unwrap()).unwrap();
        assert_eq!(p.as_px(), 3.5);
    }

    #[test]
    fn constructors_reject_non_finite_with_errors() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(Millimeters::try_new(bad), Err(NonFiniteError));
            assert_eq!(LogicalPixels::try_new(bad), Err(NonFiniteError));
        }
    }

    #[test]
    fn deserialize_rejects_non_finite() {
        assert!(Millimeters::deserialize(UnitDeserializer::new(f32::NAN)).is_err());
        assert!(Millimeters::deserialize(UnitDeserializer::new(f32::INFINITY)).is_err());
        assert!(LogicalPixels::deserialize(UnitDeserializer::new(f32::NEG_INFINITY)).is_err());
        let mm = Millimeters::deserialize(UnitDeserializer::new(2.5)).unwrap();
        assert_eq!(mm.as_mm(), 2.5);
    }

    #[test]
    fn serde_json_round_trip() {
        let mm = Millimeters::try_new(1.25).unwrap();
        let json = serde_json::to_string(&mm).unwrap();
        assert_eq!(serde_json::from_str::<Millimeters>(&json).unwrap(), mm);
    }

    #[test]
    fn checked_arithmetic_preserves_finiteness() {
        let one = Millimeters::try_new(1.0).unwrap();
        let two = Millimeters::try_new(2.0).unwrap();
        let two_point_five = Millimeters::try_new(2.5).unwrap();
        assert_eq!(one.checked_add(two).unwrap().as_mm(), 3.0);
        assert_eq!(one.checked_sub(two_point_five).unwrap().as_mm(), -1.5);
        assert_eq!(
            one.checked_sub(two),
            Some(Millimeters::try_new(-1.0).unwrap())
        );
        // Overflow would produce infinity -> None, never a panic.
        let max = Millimeters::try_new(f32::MAX).unwrap();
        assert_eq!(max.checked_add(max), None);
        assert_eq!(
            LogicalPixels::try_new(f32::MAX)
                .unwrap()
                .checked_add(LogicalPixels::try_new(f32::MAX).unwrap()),
            None
        );
        // A small addend at the top of the range is absorbed by f32's ULP and
        // stays finite; it must still be reported as a valid checked result.
        assert_eq!(max.checked_add(one), Some(max));
    }

    #[test]
    fn px_per_mm_scale_rejects_invalid_values() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0, -0.5] {
            assert_eq!(
                LogicalPixelsPerMm::try_new(bad),
                Err(ScaleError),
                "bad: {bad}"
            );
        }
        let scale = LogicalPixelsPerMm::try_new(10.0).unwrap();
        assert_eq!(scale.as_px_per_mm(), 10.0);
        assert!(scale.is_finite());
        assert_eq!(scale.to_string(), "10px/mm");
        assert_eq!(
            LogicalPixelsPerMm::try_new(2.5).unwrap(),
            LogicalPixelsPerMm::try_new(2.5).unwrap()
        );
        assert_ne!(
            LogicalPixelsPerMm::try_new(2.5).unwrap(),
            LogicalPixelsPerMm::try_new(3.0).unwrap()
        );
    }
}
