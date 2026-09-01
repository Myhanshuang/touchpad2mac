//! M18 configurable gesture mapping profile.

#![forbid(unsafe_code)]

use crate::{
    ArbiterConfig, M17Profile, M17ProfileError, RobustnessConfigError, UserSettings,
    UserSettingsError,
};

pub const M18_REMAP_V1_NAME: &str = "m18-remap-v1";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M18ProfileError {
    #[error("invalid M18 user settings: {0}")]
    Settings(UserSettingsError),
    #[error("invalid M17 base profile: {0}")]
    M17(M17ProfileError),
    #[error("invalid DWT robustness overlay: {0}")]
    Robustness(RobustnessConfigError),
    #[error("M17 base profile is missing the M15 three-finger drag stage")]
    MissingThreeFingerDrag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct M18Profile {
    settings: UserSettings,
    base: M17Profile,
}

impl M18Profile {
    pub const NAME: &str = M18_REMAP_V1_NAME;

    pub fn new(settings: UserSettings) -> Result<Self, M18ProfileError> {
        settings.validate().map_err(M18ProfileError::Settings)?;
        let base = M17Profile::with_feel(settings.feel.clone()).map_err(M18ProfileError::M17)?;
        Ok(Self { settings, base })
    }

    pub fn default_profile() -> Result<Self, M18ProfileError> {
        Self::new(UserSettings::default())
    }

    #[must_use]
    pub const fn settings(&self) -> &UserSettings {
        &self.settings
    }

    pub fn arbiter_config(&self) -> Result<ArbiterConfig, M18ProfileError> {
        let mut base = self.base.arbiter_config().map_err(M18ProfileError::M17)?;
        if let Some(robustness) = base.robustness_config().cloned() {
            let robustness = robustness
                .with_dwt(self.settings.dwt.clone())
                .map_err(M18ProfileError::Robustness)?;
            base = base.with_robustness(robustness);
        }
        let drag = base
            .three_finger_drag_config()
            .cloned()
            .ok_or(M18ProfileError::MissingThreeFingerDrag)?
            .with_drag_enabled(self.settings.gestures.three_finger_drag_enabled);
        Ok(base
            .with_three_finger_drag(drag)
            .with_gesture_bindings(self.settings.gestures.clone()))
    }
}
