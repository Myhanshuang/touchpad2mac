//! The M10 bring-up policy profile `m10-linear-v1` (M10_TASK.md §3).
//!
//! M10 requires **one named, versioned profile** whose every M7–M9 parameter
//! is typed, finite, validated, and documented. [`M10Profile`] is that
//! profile: it is a **conservative bring-up profile** for the first bounded
//! takeover slice — explicitly **not** a macOS-equivalence claim and **not**
//! a production default. It must not be read from or copied from KDE/libinput
//! settings at runtime; the current system behavior is only the manual A/B
//! baseline (`doc/old/acceptance/M6_ACCEPTANCE.md` §3).
//!
//! # What the profile configures
//!
//! Every M7–M9 parameter is present and validated through the existing typed
//! constructors ([`ArbiterConfig::new`], [`TapConfig::new`],
//! [`TwoFingerConfig::new`], [`LogicalPixelsPerMm::try_new`]), so no value can
//! be non-finite, non-positive, zero, or an impossible feature combination:
//!
//! * **One-finger linear pointer** — commit threshold and linear mm→logical
//!   pixel scale (M7).
//! * **Tap-to-click / tap-and-drag / sticky drag lock** — enable flags plus
//!   the tap duration, tap movement limit, and follow-up gap (M8).
//! * **Two-finger 2D natural scroll** — enable flag, explicit natural
//!   direction, scroll scale, and centroid commit threshold (M9).
//! * **Secondary (two-finger) tap** — enable flag, duration and per-contact
//!   movement limits (M9).
//! * **Buttonpad two-finger physical secondary click** — enable flag (M9).
//!
//! # What the profile explicitly does NOT add
//!
//! No acceleration, momentum, palm/thumb classification, pinch/rotate/
//! swipes, Force Click, pressure, or haptics — those are later-milestone
//! behaviors (PHASE2_PLAN.md §4) and are deliberately absent from
//! `m10-linear-v1`.
//!
//! # Status
//!
//! This profile is only a bring-up configuration. It does **not** qualify
//! the live desktop output backend and does **not** constitute measurement
//! evidence for the M6 calibration gate (`--output-qualified` is an operator
//! attestation, not data; see `doc/old/acceptance/M10_ACCEPTANCE.md`).

use std::time::Duration;

use crate::arbiter::{
    ArbiterConfig, TapConfig, TapConfigError, TwoFingerConfig, TwoFingerConfigError,
};
use crate::units::{LogicalPixelsPerMm, Millimeters};

/// The exact versioned profile name accepted by `touchpadctl takeover
/// --profile`.
pub const M10_LINEAR_V1_NAME: &str = "m10-linear-v1";

/// Failure of [`M10Profile::new`] — only reachable when a documented
/// constant fails its own validation (a programming error; the constants are
/// chosen to validate).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum M10ProfileError {
    /// The tap configuration was rejected.
    #[error("m10-linear-v1 tap configuration is invalid: {0}")]
    Tap(TapConfigError),
    /// The two-finger configuration was rejected.
    #[error("m10-linear-v1 two-finger configuration is invalid: {0}")]
    TwoFinger(TwoFingerConfigError),
}

/// The `m10-linear-v1` bring-up profile: every M7–M9 parameter typed,
/// finite, validated (at construction), and documented.
///
/// All values are conservative bring-up constants — chosen to be safely
/// usable, not tuned to match any macOS or KDE behavior. They are never read
/// from the system at runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct M10Profile {
    motion_threshold_mm: Millimeters,
    logical_pixels_per_mm: LogicalPixelsPerMm,
    tap: TapConfig,
    two_finger: TwoFingerConfig,
}

impl M10Profile {
    /// The versioned profile name (`m10-linear-v1`).
    pub const NAME: &str = M10_LINEAR_V1_NAME;

    /// Constructs the validated profile.
    ///
    /// # Errors
    ///
    /// Returns [`M10ProfileError`] if a documented constant fails its
    /// validation (impossible with the current constants, but the fallible
    /// constructors are never bypassed).
    pub fn new() -> Result<Self, M10ProfileError> {
        // M7: one-finger pointer — 1.0 mm commit threshold, 10 px/mm linear
        // scale. Conservative: a deliberate, low-gain bring-up mapping.
        let motion_threshold_mm = Millimeters::try_new(1.0).expect("documented constant");
        let logical_pixels_per_mm = LogicalPixelsPerMm::try_new(10.0).expect("documented constant");

        // M8: tap-to-click, tap-and-drag and sticky drag lock enabled with
        // conservative timing: 180 ms maximum tap duration (equality
        // accepted, strictly longer cancels), 3.0 mm maximum tap movement
        // (equality accepted, strictly farther disqualifies), 350 ms
        // follow-up gap after a qualifying tap.
        let tap = TapConfig::new(
            true, // tap_enabled
            true, // tap_and_drag_enabled
            true, // drag_lock_enabled
            Duration::from_millis(180),
            Millimeters::try_new(3.0).expect("documented constant"),
            Duration::from_millis(350),
        )
        .map_err(M10ProfileError::Tap)?;

        // M9: two-finger 2D natural pixel scroll (10 px/mm, 1.0 mm centroid
        // commit threshold, natural direction on), two-finger secondary tap
        // (300 ms maximum, 3.0 mm per-contact movement), and buttonpad
        // two-finger physical secondary click, all enabled.
        let two_finger = TwoFingerConfig::new(
            true, // scroll_enabled
            true, // natural
            LogicalPixelsPerMm::try_new(10.0).expect("documented constant"),
            Millimeters::try_new(1.0).expect("documented constant"),
            true, // secondary_tap_enabled
            true, // two_finger_physical_click_enabled
            Duration::from_millis(300),
            Millimeters::try_new(3.0).expect("documented constant"),
        )
        .map_err(M10ProfileError::TwoFinger)?;

        Ok(Self {
            motion_threshold_mm,
            logical_pixels_per_mm,
            tap,
            two_finger,
        })
    }

    /// The one-finger pointer commit threshold (M7).
    #[must_use]
    pub const fn motion_threshold_mm(&self) -> Millimeters {
        self.motion_threshold_mm
    }

    /// The one-finger linear mm→logical-pixel scale (M7).
    #[must_use]
    pub const fn logical_pixels_per_mm(&self) -> LogicalPixelsPerMm {
        self.logical_pixels_per_mm
    }

    /// The M8 tap/tap-and-drag/drag-lock configuration.
    #[must_use]
    pub const fn tap(&self) -> &TapConfig {
        &self.tap
    }

    /// The M9 two-finger scroll/secondary-tap/buttonpad-click configuration.
    #[must_use]
    pub const fn two_finger(&self) -> &TwoFingerConfig {
        &self.two_finger
    }

    /// Builds the validated [`ArbiterConfig`] the takeover pipeline runs
    /// with.
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        ArbiterConfig::new(self.motion_threshold_mm, self.logical_pixels_per_mm)
            .expect("validated at construction")
            .with_tap(self.tap.clone())
            .with_two_finger(self.two_finger.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_constructs_and_exposes_every_parameter() {
        let profile = M10Profile::new().expect("documented constants must validate");
        assert_eq!(M10Profile::NAME, "m10-linear-v1");
        assert!(profile.motion_threshold_mm().as_mm() > 0.0);
        assert!(profile.logical_pixels_per_mm().as_px_per_mm() > 0.0);
        // M8 tap family enabled with strictly positive limits.
        assert!(profile.tap().tap_enabled());
        assert!(profile.tap().tap_and_drag_enabled());
        assert!(profile.tap().drag_lock_enabled());
        assert!(!profile.tap().max_tap_duration().is_zero());
        assert!(profile.tap().max_tap_movement_mm().as_mm() > 0.0);
        assert!(!profile.tap().max_tap_drag_gap().is_zero());
        // M9 two-finger family enabled with strictly positive limits.
        assert!(profile.two_finger().scroll_enabled());
        assert!(profile.two_finger().natural());
        assert!(profile.two_finger().secondary_tap_enabled());
        assert!(profile.two_finger().two_finger_physical_click_enabled());
        assert!(
            profile
                .two_finger()
                .scroll_logical_pixels_per_mm()
                .as_px_per_mm()
                > 0.0
        );
        assert!(profile.two_finger().scroll_commit_threshold_mm().as_mm() > 0.0);
        assert!(!profile.two_finger().max_secondary_tap_duration().is_zero());
        assert!(profile.two_finger().max_secondary_tap_movement_mm().as_mm() > 0.0);
    }

    #[test]
    fn profile_builds_a_validated_arbiter_config() {
        let profile = M10Profile::new().unwrap();
        let config = profile.arbiter_config();
        assert_eq!(config.motion_threshold_mm(), profile.motion_threshold_mm());
        assert_eq!(
            config.logical_pixels_per_mm(),
            profile.logical_pixels_per_mm()
        );
        assert_eq!(config.tap_config(), Some(profile.tap()));
        assert_eq!(config.two_finger_config(), Some(profile.two_finger()));
        assert!(config.is_tap_enabled());
        assert!(config.is_two_finger_enabled());
    }

    #[test]
    fn profile_name_matches_the_cli_contract() {
        assert_eq!(M10Profile::NAME, "m10-linear-v1");
    }
}
