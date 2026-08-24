//! M16 productionization profile `m16-production-v1`.
//!
//! The name means the M12-M16 configuration stack is present. It does not
//! imply live or cross-device production qualification.

#![forbid(unsafe_code)]

use crate::{ArbiterConfig, M15Profile, M15ProfileError};

/// Versioned M16 profile name.
pub const M16_PRODUCTION_V1_NAME: &str = "m16-production-v1";

/// M16 profile construction failure.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M16ProfileError {
    /// The inherited M15 profile failed its own validation.
    #[error("m16-production-v1 base M15 profile is invalid: {0}")]
    M15(M15ProfileError),
}

/// M16 final Phase-2 profile. No interaction constants are copied here.
#[derive(Clone, Debug, PartialEq)]
pub struct M16Profile {
    base: M15Profile,
}

impl M16Profile {
    /// Canonical versioned profile name.
    pub const NAME: &str = M16_PRODUCTION_V1_NAME;

    /// Constructs the inherited validated profile.
    pub fn new() -> Result<Self, M16ProfileError> {
        let base = M15Profile::new().map_err(M16ProfileError::M15)?;
        Ok(Self { base })
    }

    /// Inherited M15 profile.
    #[must_use]
    pub const fn m15_profile(&self) -> &M15Profile {
        &self.base
    }

    /// Builds the exact inherited interaction configuration.
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base.arbiter_config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m16_inherits_m15_without_changing_interaction_policy() {
        let m15 = M15Profile::new().unwrap();
        let m16 = M16Profile::new().unwrap();
        assert_eq!(M16Profile::NAME, "m16-production-v1");
        assert_eq!(m16.m15_profile(), &m15);
        assert_eq!(m16.arbiter_config(), m15.arbiter_config());
    }
}
