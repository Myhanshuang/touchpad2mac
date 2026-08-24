//! The M11 experimental one-finger pointer-fidelity profile
//! `m11-fidelity-v1` (M11_TASK.md §10).
//!
//! M11 is **experimental, opt-in only, never the default, and makes no macOS
//! equivalence claim**. It adds a platform-independent pointer-fidelity stage
//! (dead zone, time-domain velocity, smoothstep gain, tracking multiplier)
//! for already normalized committed one-finger millimeter motion, layered on
//! top of the approved M7–M9 interaction policy from the `m10-linear-v1`
//! bring-up profile.
//!
//! # Configuration
//!
//! [`M11Profile`] obtains the entire M7–M9 configuration from
//! [`M10Profile::new()`](crate::M10Profile::new) — never copying M7–M9
//! constants — and adds one validated [`FidelityConfig`]. The provisional M11
//! constants live only here (the single source location):
//!
//! | Parameter | Value |
//! | --- | ---: |
//! | dead zone radius | `0.09 mm` |
//! | velocity time constant | `20 ms` |
//! | long gap | `150 ms` (inclusive boundary) |
//! | gain curve x0 | `50 mm/s` |
//! | gain curve x1 | `600 mm/s` |
//! | minimum gain | `1.0` |
//! | maximum gain | `2.0` |
//! | base scale | `10 px/mm` (the M10 one-finger scale) |
//! | tracking multiplier | `1.0` |
//!
//! # Status
//!
//! M11 is **live-unqualified**: it remains unvalidated on a real desktop
//! until a separate, later M11-specific user acceptance is written and
//! passed. M10 acceptance does not qualify M11, and `--output-qualified`
//! stays an operator attestation rather than measurement evidence.

use crate::arbiter::ArbiterConfig;
use crate::fidelity::{FidelityConfig, FidelityConfigError};
use crate::m10::{M10Profile, M10ProfileError};

/// The exact versioned profile name accepted by `touchpadctl takeover
/// --profile` for M11.
pub const M11_FIDELITY_V1_NAME: &str = "m11-fidelity-v1";

/// Failure of [`M11Profile::new`] — only reachable when a documented
/// constant fails its own validation (a programming error; the constants are
/// chosen to validate).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum M11ProfileError {
    /// The underlying M10 profile could not be constructed.
    #[error("m11-fidelity-v1 base M10 profile is invalid: {0}")]
    M10(M10ProfileError),
    /// The fidelity configuration was rejected.
    #[error("m11-fidelity-v1 fidelity configuration is invalid: {0}")]
    Fidelity(FidelityConfigError),
}

/// The `m11-fidelity-v1` experimental one-finger pointer-fidelity profile.
///
/// It derives every M7–M9 value from [`M10Profile`] and adds the validated
/// [`FidelityConfig`]. All values are typed, finite, validated at
/// construction, versioned, documented in one source location, and never
/// loaded from KDE/libinput.
#[derive(Clone, Debug, PartialEq)]
pub struct M11Profile {
    base: M10Profile,
    fidelity: FidelityConfig,
}

impl M11Profile {
    /// The versioned profile name (`m11-fidelity-v1`).
    pub const NAME: &str = M11_FIDELITY_V1_NAME;

    /// Constructs the validated profile.
    ///
    /// # Errors
    ///
    /// Returns [`M11ProfileError`] if a documented constant fails its
    /// validation (impossible with the current constants, but the fallible
    /// constructors are never bypassed).
    pub fn new() -> Result<Self, M11ProfileError> {
        // The whole M7–M9 configuration is inherited from M10; no constant is
        // copied here.
        let base = M10Profile::new().map_err(M11ProfileError::M10)?;

        // M11 provisional fidelity constants (M11_TASK.md §10). 0.09 mm
        // jitter dead zone, 20 ms velocity time constant, 150 ms inclusive
        // long gap, 50..600 mm/s smoothstep gain curve, gain 1.0..2.0, the
        // M10 base scale, tracking multiplier 1.0.
        //
        // R4 (review): propagate a rejected fidelity configuration as
        // `M11ProfileError::Fidelity` instead of panicking, so the
        // constructor's `Result` contract holds even if a future edit makes a
        // documented constant invalid.
        let fidelity = FidelityConfig::new(
            0.09,
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(150),
            50.0,
            600.0,
            1.0,
            2.0,
            base.logical_pixels_per_mm(),
            1.0,
        )
        .map_err(M11ProfileError::Fidelity)?;

        Ok(Self { base, fidelity })
    }

    /// The validated [`FidelityConfig`] this profile adds.
    #[must_use]
    pub const fn fidelity_config(&self) -> &FidelityConfig {
        &self.fidelity
    }

    /// The inherited `m10-linear-v1` profile.
    #[must_use]
    pub const fn m10_profile(&self) -> &M10Profile {
        &self.base
    }

    /// The one-finger pointer commit threshold (inherited from M10/M7).
    #[must_use]
    pub const fn motion_threshold_mm(&self) -> crate::units::Millimeters {
        self.base.motion_threshold_mm()
    }

    /// The one-finger linear mm→logical-pixel scale (inherited from M10/M7;
    /// also the fidelity base scale).
    #[must_use]
    pub const fn logical_pixels_per_mm(&self) -> crate::units::LogicalPixelsPerMm {
        self.base.logical_pixels_per_mm()
    }

    /// The M8 tap/tap-and-drag/drag-lock configuration (inherited from M10).
    #[must_use]
    pub const fn tap(&self) -> &crate::arbiter::TapConfig {
        self.base.tap()
    }

    /// The M9 two-finger configuration (inherited from M10).
    #[must_use]
    pub const fn two_finger(&self) -> &crate::arbiter::TwoFingerConfig {
        self.base.two_finger()
    }

    /// Builds the validated [`ArbiterConfig`] the takeover pipeline runs
    /// with: the inherited M10 configuration plus the M11 fidelity stage.
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base
            .arbiter_config()
            .with_fidelity(self.fidelity.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_constructs_and_exposes_every_parameter() {
        let profile = M11Profile::new().expect("documented constants must validate");
        assert_eq!(M11Profile::NAME, "m11-fidelity-v1");
        assert_eq!(M11_FIDELITY_V1_NAME, "m11-fidelity-v1");
        // The fidelity config exposes the exact M11 constants.
        let fidelity = profile.fidelity_config();
        assert_eq!(fidelity.dead_zone_radius_mm(), 0.09);
        assert_eq!(
            fidelity.velocity_tau(),
            std::time::Duration::from_millis(20)
        );
        assert_eq!(fidelity.long_gap(), std::time::Duration::from_millis(150));
        assert_eq!(fidelity.gain_x0_mm_per_s(), 50.0);
        assert_eq!(fidelity.gain_x1_mm_per_s(), 600.0);
        assert_eq!(fidelity.min_gain(), 1.0);
        assert_eq!(fidelity.max_gain(), 2.0);
        assert_eq!(fidelity.tracking_speed(), 1.0);
    }

    #[test]
    fn profile_inherits_every_m10_value() {
        let m10 = M10Profile::new().unwrap();
        let m11 = M11Profile::new().unwrap();
        assert_eq!(m11.motion_threshold_mm(), m10.motion_threshold_mm());
        assert_eq!(m11.logical_pixels_per_mm(), m10.logical_pixels_per_mm());
        assert_eq!(m11.tap(), m10.tap());
        assert_eq!(m11.two_finger(), m10.two_finger());
        assert_eq!(m11.m10_profile(), &m10);
    }

    #[test]
    fn profile_builds_a_validated_arbiter_config_with_fidelity() {
        let profile = M11Profile::new().unwrap();
        let config = profile.arbiter_config();
        // Inherited M10/M7–M9 values.
        assert_eq!(config.motion_threshold_mm(), profile.motion_threshold_mm());
        assert_eq!(
            config.logical_pixels_per_mm(),
            profile.logical_pixels_per_mm()
        );
        assert_eq!(config.tap_config(), Some(profile.tap()));
        assert_eq!(config.two_finger_config(), Some(profile.two_finger()));
        assert!(config.is_tap_enabled());
        assert!(config.is_two_finger_enabled());
        // Fidelity enabled, carrying the M11 config.
        assert!(config.is_fidelity_enabled());
        assert_eq!(config.fidelity_config(), Some(profile.fidelity_config()));
    }

    #[test]
    fn profile_name_matches_the_cli_contract() {
        assert_eq!(M11Profile::NAME, "m11-fidelity-v1");
    }

    #[test]
    fn fidelity_error_variant_is_reachable() {
        // R4 guard: `M11Profile::new` propagates a rejected fidelity
        // configuration as `M11ProfileError::Fidelity` instead of panicking,
        // so the variant must stay live and render its underlying config
        // error (the documented `Result` contract, not a panic path).
        let err = M11ProfileError::Fidelity(FidelityConfigError::DeadZoneRadius(-1.0));
        assert!(matches!(
            err,
            M11ProfileError::Fidelity(FidelityConfigError::DeadZoneRadius(v)) if v == -1.0
        ));
        assert!(err
            .to_string()
            .contains("m11-fidelity-v1 fidelity configuration is invalid"));
        assert!(err
            .to_string()
            .contains("fidelity dead zone radius must be finite and strictly positive"));
    }
}
