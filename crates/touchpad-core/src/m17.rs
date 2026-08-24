//! M17 explicitly tunable profile `m17-tunable-v1`.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{
    ArbiterConfig, FeelConfig, FeelConfigError, FidelityConfig, FidelityConfigError, GestureConfig,
    GestureConfigError, M16Profile, M16ProfileError, ScrollFidelityConfig,
    ScrollFidelityConfigError, ThreeFingerDragConfig, ThreeFingerDragConfigError,
};

pub const M17_TUNABLE_V1_NAME: &str = "m17-tunable-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M17ProfileError {
    #[error("m17-tunable-v1 base M16 profile is invalid: {0}")]
    M16(M16ProfileError),
    #[error("invalid M17 feel overlay: {0}")]
    Feel(FeelConfigError),
    #[error("M17 pointer configuration is invalid: {0}")]
    Pointer(FidelityConfigError),
    #[error("M17 scroll configuration is invalid: {0}")]
    Scroll(ScrollFidelityConfigError),
    #[error("M17 gesture configuration is invalid: {0}")]
    Gesture(GestureConfigError),
    #[error("M17 drag configuration is invalid: {0}")]
    Drag(ThreeFingerDragConfigError),
    #[error("M16 profile is missing an expected {0} stage")]
    MissingBaseStage(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M17Profile {
    base: M16Profile,
    feel: FeelConfig,
}

impl M17Profile {
    pub const NAME: &str = M17_TUNABLE_V1_NAME;

    pub fn new() -> Result<Self, M17ProfileError> {
        Self::with_feel(FeelConfig::default())
    }

    pub fn with_feel(feel: FeelConfig) -> Result<Self, M17ProfileError> {
        feel.validate().map_err(M17ProfileError::Feel)?;
        let base = M16Profile::new().map_err(M17ProfileError::M16)?;
        let profile = Self { base, feel };
        profile.build_config()?;
        Ok(profile)
    }

    #[must_use]
    pub const fn feel(&self) -> &FeelConfig {
        &self.feel
    }

    #[must_use]
    pub const fn m16_profile(&self) -> &M16Profile {
        &self.base
    }

    pub fn arbiter_config(&self) -> Result<ArbiterConfig, M17ProfileError> {
        self.build_config()
    }

    fn build_config(&self) -> Result<ArbiterConfig, M17ProfileError> {
        let base = self.base.arbiter_config();
        let pointer = base
            .fidelity_config()
            .ok_or(M17ProfileError::MissingBaseStage("pointer fidelity"))?
            .clone();
        let scroll = base
            .scroll_fidelity_config()
            .ok_or(M17ProfileError::MissingBaseStage("scroll fidelity"))?
            .clone();
        let gesture = base
            .gesture_config()
            .ok_or(M17ProfileError::MissingBaseStage("gesture"))?
            .clone();
        let drag = base
            .three_finger_drag_config()
            .ok_or(M17ProfileError::MissingBaseStage("three-finger drag"))?
            .clone();

        let pointer = FidelityConfig::new(
            self.feel.pointer.dead_zone_radius_mm,
            pointer.velocity_tau(),
            pointer.long_gap(),
            pointer.gain_x0_mm_per_s(),
            pointer.gain_x1_mm_per_s(),
            self.feel.pointer.min_gain,
            self.feel.pointer.max_gain,
            pointer.base_px_per_mm(),
            self.feel.pointer.tracking_speed,
        )
        .map_err(M17ProfileError::Pointer)?;

        let scroll = ScrollFidelityConfig::new(
            scroll.velocity_tau(),
            scroll.gain_x0_mm_per_s(),
            scroll.gain_x1_mm_per_s(),
            self.feel.scroll.min_gain,
            self.feel.scroll.max_gain,
            self.feel.scroll.axis_lock_engage_ratio,
            self.feel.scroll.axis_lock_release_ratio,
            Duration::from_millis(self.feel.scroll.momentum_tau_ms),
            self.feel.scroll.momentum_start_speed_mm_per_s,
            self.feel.scroll.momentum_stop_speed_mm_per_s,
            scroll.momentum_tick_cap(),
        )
        .map_err(M17ProfileError::Scroll)?;

        let mut gesture_new = GestureConfig::new(
            self.feel.gesture.pinch_commit_mm,
            gesture.rotate_commit_radians(),
            self.feel.gesture.page_swipe_commit_mm,
            gesture.page_swipe_dominance(),
            gesture.scroll_translation_win_mm(),
            self.feel.gesture.multi_swipe_commit_mm,
            gesture.edge_commit_mm(),
            gesture.edge_zone_mm(),
        )
        .map_err(M17ProfileError::Gesture)?;
        if let Some((width, height)) = gesture.surface_size_mm() {
            gesture_new = gesture_new
                .with_surface_size_mm(width, height)
                .map_err(M17ProfileError::Gesture)?;
        }

        let drag = ThreeFingerDragConfig::new(
            self.feel.drag.commit_threshold_mm,
            drag.tap_max_displacement_mm(),
            drag.tap_max_duration(),
            self.feel.drag.drag_lock,
        )
        .map_err(M17ProfileError::Drag)?;

        Ok(base
            .with_fidelity(pointer)
            .with_scroll_fidelity(scroll)
            .with_gesture(gesture_new)
            .with_three_finger_drag(drag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_feel_is_exactly_m16_equivalent() {
        let m16 = M16Profile::new().unwrap();
        let m17 = M17Profile::new().unwrap();
        assert_eq!(m17.arbiter_config().unwrap(), m16.arbiter_config());
    }

    #[test]
    fn tuning_changes_only_the_targeted_underlying_stages() {
        let m16 = M16Profile::new().unwrap().arbiter_config();
        let mut feel = FeelConfig::default();
        feel.set_key("pointer.tracking_speed", "1.5").unwrap();
        feel.set_key("scroll.momentum_tau_ms", "500").unwrap();
        feel.set_key("gesture.pinch_commit_mm", "1.2").unwrap();
        feel.set_key("drag.commit_threshold_mm", "1.2").unwrap();
        let tuned = M17Profile::with_feel(feel)
            .unwrap()
            .arbiter_config()
            .unwrap();
        assert_eq!(tuned.tap_config(), m16.tap_config());
        assert_eq!(tuned.two_finger_config(), m16.two_finger_config());
        assert_eq!(tuned.robustness_config(), m16.robustness_config());
        assert_ne!(tuned.fidelity_config(), m16.fidelity_config());
        assert_ne!(tuned.scroll_fidelity_config(), m16.scroll_fidelity_config());
        assert_ne!(tuned.gesture_config(), m16.gesture_config());
        assert_ne!(
            tuned.three_finger_drag_config(),
            m16.three_finger_drag_config()
        );
    }
}
