//! M14 continuous gesture profile `m14-gestures-v1`.

#![forbid(unsafe_code)]

use crate::{ArbiterConfig, GestureConfig, GestureConfigError, M13Profile, M13ProfileError};

pub const M14_GESTURES_V1_NAME: &str = "m14-gestures-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M14ProfileError {
    #[error("m14-gestures-v1 base M13 profile is invalid: {0}")]
    M13(M13ProfileError),
    #[error("m14-gestures-v1 gesture configuration is invalid: {0}")]
    Gesture(GestureConfigError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M14Profile {
    base: M13Profile,
    gesture: GestureConfig,
}

impl M14Profile {
    pub const NAME: &str = M14_GESTURES_V1_NAME;
    pub fn new() -> Result<Self, M14ProfileError> {
        let base = M13Profile::new().map_err(M14ProfileError::M13)?;
        let gesture = GestureConfig::new(0.8, 0.15, 0.8, 4.0, 1.0, 2.0, 2.0, 3.0)
            .map_err(M14ProfileError::Gesture)?;
        Ok(Self { base, gesture })
    }
    #[must_use]
    pub const fn gesture_config(&self) -> &GestureConfig {
        &self.gesture
    }
    #[must_use]
    pub const fn m13_profile(&self) -> &M13Profile {
        &self.base
    }
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base
            .arbiter_config()
            .with_gesture(self.gesture.clone())
    }
}
