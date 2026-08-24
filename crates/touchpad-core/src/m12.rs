//! M12 experimental scroll-fidelity and momentum profile `m12-scroll-v1`.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::arbiter::ArbiterConfig;
use crate::m11::{M11Profile, M11ProfileError};
use crate::scroll_fidelity::{ScrollFidelityConfig, ScrollFidelityConfigError};

pub const M12_SCROLL_V1_NAME: &str = "m12-scroll-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M12ProfileError {
    #[error("m12-scroll-v1 base M11 profile is invalid: {0}")]
    M11(M11ProfileError),
    #[error("m12-scroll-v1 scroll-fidelity configuration is invalid: {0}")]
    ScrollFidelity(ScrollFidelityConfigError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M12Profile {
    base: M11Profile,
    scroll_fidelity: ScrollFidelityConfig,
}

impl M12Profile {
    pub const NAME: &str = M12_SCROLL_V1_NAME;

    pub fn new() -> Result<Self, M12ProfileError> {
        let base = M11Profile::new().map_err(M12ProfileError::M11)?;
        let scroll_fidelity = ScrollFidelityConfig::new(
            Duration::from_millis(30),
            25.0,
            450.0,
            1.0,
            1.75,
            2.5,
            1.5,
            Duration::from_millis(325),
            35.0,
            6.0,
            Duration::from_millis(16),
        )
        .map_err(M12ProfileError::ScrollFidelity)?;
        Ok(Self {
            base,
            scroll_fidelity,
        })
    }

    #[must_use]
    pub const fn m11_profile(&self) -> &M11Profile {
        &self.base
    }

    #[must_use]
    pub const fn scroll_fidelity_config(&self) -> &ScrollFidelityConfig {
        &self.scroll_fidelity
    }

    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base
            .arbiter_config()
            .with_scroll_fidelity(self.scroll_fidelity.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_versioned_parameters_and_inheritance() {
        let m11 = M11Profile::new().unwrap();
        let m12 = M12Profile::new().unwrap();
        assert_eq!(M12Profile::NAME, "m12-scroll-v1");
        assert_eq!(m12.m11_profile(), &m11);
        let cfg = m12.scroll_fidelity_config();
        assert_eq!(cfg.velocity_tau(), Duration::from_millis(30));
        assert_eq!(cfg.gain_x0_mm_per_s(), 25.0);
        assert_eq!(cfg.gain_x1_mm_per_s(), 450.0);
        assert_eq!(cfg.min_gain(), 1.0);
        assert_eq!(cfg.max_gain(), 1.75);
        assert_eq!(cfg.axis_lock_engage_ratio(), 2.5);
        assert_eq!(cfg.axis_lock_release_ratio(), 1.5);
        assert_eq!(cfg.momentum_tau(), Duration::from_millis(325));
        assert_eq!(cfg.momentum_start_speed_mm_per_s(), 35.0);
        assert_eq!(cfg.momentum_stop_speed_mm_per_s(), 6.0);
        assert_eq!(cfg.momentum_tick_cap(), Duration::from_millis(16));

        let m11_cfg = m11.arbiter_config();
        let m12_cfg = m12.arbiter_config();
        assert_eq!(m12_cfg.motion_threshold_mm(), m11_cfg.motion_threshold_mm());
        assert_eq!(
            m12_cfg.logical_pixels_per_mm(),
            m11_cfg.logical_pixels_per_mm()
        );
        assert_eq!(m12_cfg.tap_config(), m11_cfg.tap_config());
        assert_eq!(m12_cfg.two_finger_config(), m11_cfg.two_finger_config());
        assert_eq!(m12_cfg.fidelity_config(), m11_cfg.fidelity_config());
        assert!(m12_cfg.is_scroll_fidelity_enabled());
        assert_eq!(
            m12_cfg.scroll_fidelity_config(),
            Some(m12.scroll_fidelity_config())
        );
    }
}
