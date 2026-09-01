//! M13 contact robustness profile `m13-robust-v1`.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{
    ArbiterConfig, DwtConfig, M12Profile, M12ProfileError, RobustnessConfig, RobustnessConfigError,
};

pub const M13_ROBUST_V1_NAME: &str = "m13-robust-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M13ProfileError {
    #[error("m13-robust-v1 base M12 profile is invalid: {0}")]
    M12(M12ProfileError),
    #[error("m13-robust-v1 robustness configuration is invalid: {0}")]
    Robustness(RobustnessConfigError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M13Profile {
    base: M12Profile,
    robustness: RobustnessConfig,
}

impl M13Profile {
    pub const NAME: &str = M13_ROBUST_V1_NAME;

    pub fn new() -> Result<Self, M13ProfileError> {
        let base = M12Profile::new().map_err(M13ProfileError::M12)?;
        // Generic defaults deliberately omit surface dimensions. Edge-start
        // suppression reports unavailable until a device boundary supplies
        // dimensions explicitly; no CIRQ-only size leaks into generic core.
        let robustness = RobustnessConfig::new(12.0, 8.0, 3.0, 0.06, Duration::from_millis(500))
            .and_then(|config| config.with_dwt(DwtConfig::default()))
            .map_err(M13ProfileError::Robustness)?;
        Ok(Self { base, robustness })
    }

    #[must_use]
    pub const fn robustness_config(&self) -> &RobustnessConfig {
        &self.robustness
    }
    #[must_use]
    pub const fn m12_profile(&self) -> &M12Profile {
        &self.base
    }
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base
            .arbiter_config()
            .with_robustness(self.robustness.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_m12_and_adds_only_robustness() {
        let m12 = M12Profile::new().unwrap();
        let m13 = M13Profile::new().unwrap();
        assert_eq!(M13Profile::NAME, "m13-robust-v1");
        assert_eq!(m13.m12_profile(), &m12);
        let base = m12.arbiter_config();
        let cfg = m13.arbiter_config();
        assert_eq!(cfg.fidelity_config(), base.fidelity_config());
        assert_eq!(cfg.scroll_fidelity_config(), base.scroll_fidelity_config());
        assert!(cfg.is_robustness_enabled());
        assert_eq!(cfg.robustness_config(), Some(m13.robustness_config()));
        assert_eq!(m13.robustness_config().surface_size_mm(), None);
    }
}
