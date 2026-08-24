//! Shared validation helpers used by unit types and contact/frame checks.
//!
//! These helpers keep finiteness and range validation in one place so that
//! constructors and `Deserialize` implementations cannot drift apart.

use serde::{de, Deserialize, Deserializer};

/// True when `value` is neither `NaN` nor infinite.
#[must_use]
pub fn is_finite_f32(value: f32) -> bool {
    value.is_finite()
}

/// True when `value` is a finite number in `[0, 1]` (e.g. normalized
/// pressure).
#[must_use]
pub fn is_normalized_f32(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// `Deserialize` implementation for a finite `f32`; rejects `NaN` and
/// infinities with a clear error.
pub fn deserialize_finite_f32<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    if is_finite_f32(value) {
        Ok(value)
    } else {
        Err(de::Error::custom(
            "expected a finite f32 (no NaN or infinity)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{Error, F32Deserializer};

    #[test]
    fn finite_and_normalized_checks() {
        assert!(is_finite_f32(0.0));
        assert!(!is_finite_f32(f32::NAN));
        assert!(!is_finite_f32(f32::INFINITY));
        assert!(is_normalized_f32(0.0));
        assert!(is_normalized_f32(1.0));
        assert!(!is_normalized_f32(1.0001));
        assert!(!is_normalized_f32(-0.0001));
        assert!(!is_normalized_f32(f32::NAN));
    }

    #[test]
    fn finite_deserialize_helper_rejects_bad_values() {
        let de = |value: f32| deserialize_finite_f32(F32Deserializer::<Error>::new(value));
        assert!(de(f32::NAN).is_err());
        assert!(de(f32::INFINITY).is_err());
        assert_eq!(de(1.5).unwrap(), 1.5);
    }
}
