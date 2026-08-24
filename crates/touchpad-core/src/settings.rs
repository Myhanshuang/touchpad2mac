//! M18 user-editable settings umbrella.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::{
    FeelConfig, FeelConfigError, GestureMapConfig, GestureMapError, GestureTarget, GestureTrigger,
};

pub const USER_SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    pub version: u32,
    pub feel: FeelConfig,
    pub gestures: GestureMapConfig,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            version: USER_SETTINGS_VERSION,
            feel: FeelConfig::default(),
            gestures: GestureMapConfig::default(),
        }
    }
}

impl UserSettings {
    pub fn validate(&self) -> Result<(), UserSettingsError> {
        if self.version != USER_SETTINGS_VERSION {
            return Err(UserSettingsError::UnsupportedVersion(self.version));
        }
        self.feel.validate().map_err(UserSettingsError::Feel)?;
        self.gestures
            .validate()
            .map_err(UserSettingsError::Gestures)?;
        Ok(())
    }

    #[must_use]
    pub fn macos_inspired() -> Self {
        Self {
            version: USER_SETTINGS_VERSION,
            feel: FeelConfig::default(),
            gestures: GestureMapConfig::macos_inspired(),
        }
    }

    /// Applies one user-facing `key=value` edit atomically.
    pub fn set_key(&mut self, key: &str, value: &str) -> Result<(), UserSettingsError> {
        let old = self.clone();
        let result = if let Some(feel_key) = key.strip_prefix("feel.") {
            self.feel
                .set_key(feel_key, value)
                .map_err(UserSettingsError::Feel)
        } else if let Some(trigger_name) = key.strip_prefix("gesture.") {
            if trigger_name == "three-finger-drag-enabled" {
                self.gestures.three_finger_drag_enabled = value
                    .parse::<bool>()
                    .map_err(|_| UserSettingsError::UnknownTarget(value.to_string()))?;
                return self.validate();
            }
            let trigger = GestureTrigger::parse(trigger_name)
                .ok_or_else(|| UserSettingsError::UnknownGesture(trigger_name.to_string()))?;
            let target = GestureTarget::parse(value)
                .ok_or_else(|| UserSettingsError::UnknownTarget(value.to_string()))?;
            self.gestures
                .set_target(trigger, target)
                .map_err(UserSettingsError::Gestures)
        } else {
            Err(UserSettingsError::UnknownKey(key.to_string()))
        };

        if let Err(error) = result {
            *self = old;
            return Err(error);
        }
        if let Err(error) = self.validate() {
            *self = old;
            return Err(error);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum UserSettingsError {
    #[error("unsupported user-settings version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid feel settings: {0}")]
    Feel(FeelConfigError),
    #[error("invalid gesture settings: {0}")]
    Gestures(GestureMapError),
    #[error("unknown settings key {0:?}")]
    UnknownKey(String),
    #[error("unknown gesture trigger {0:?}")]
    UnknownGesture(String),
    #[error("unknown gesture target {0:?}")]
    UnknownTarget(String),
}
