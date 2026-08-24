//! M15 three-finger drag / KDE action profile `m15-kde-v1`.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{
    ArbiterConfig, M14Profile, M14ProfileError, ThreeFingerDragConfig, ThreeFingerDragConfigError,
};

pub const M15_KDE_V1_NAME: &str = "m15-kde-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M15ProfileError {
    #[error("m15-kde-v1 base M14 profile is invalid: {0}")]
    M14(M14ProfileError),
    #[error("m15-kde-v1 three-finger drag config is invalid: {0}")]
    Drag(ThreeFingerDragConfigError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M15Profile {
    base: M14Profile,
    drag: ThreeFingerDragConfig,
}

impl M15Profile {
    pub const NAME: &str = M15_KDE_V1_NAME;
    pub fn new() -> Result<Self, M15ProfileError> {
        let base = M14Profile::new().map_err(M15ProfileError::M14)?;
        let drag = ThreeFingerDragConfig::new(1.0, 0.5, Duration::from_millis(200), true)
            .map_err(M15ProfileError::Drag)?;
        Ok(Self { base, drag })
    }
    #[must_use]
    pub const fn drag_config(&self) -> &ThreeFingerDragConfig {
        &self.drag
    }
    #[must_use]
    pub const fn m14_profile(&self) -> &M14Profile {
        &self.base
    }
    #[must_use]
    pub fn arbiter_config(&self) -> ArbiterConfig {
        self.base
            .arbiter_config()
            .with_three_finger_drag(self.drag.clone())
    }
}
