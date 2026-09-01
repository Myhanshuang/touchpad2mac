//! M18 user-editable settings umbrella.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

use crate::{
    DwtConfig, DwtConfigError, FeelConfig, FeelConfigError, GestureMapConfig, GestureMapError,
    GestureTarget, GestureTrigger,
};

pub const USER_SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    pub version: u32,
    pub feel: FeelConfig,
    pub gestures: GestureMapConfig,
    /// Disable-while-typing policy. `serde(default)` keeps existing v1 files
    /// valid while adding the new optional section.
    #[serde(default)]
    pub dwt: DwtConfig,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            version: USER_SETTINGS_VERSION,
            feel: FeelConfig::default(),
            gestures: GestureMapConfig::default(),
            dwt: DwtConfig::default(),
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
        self.dwt.validate().map_err(UserSettingsError::Dwt)?;
        Ok(())
    }

    #[must_use]
    pub fn macos_inspired() -> Self {
        Self {
            version: USER_SETTINGS_VERSION,
            feel: FeelConfig::default(),
            gestures: GestureMapConfig::macos_inspired(),
            dwt: DwtConfig::default(),
        }
    }

    /// Applies one user-facing `key=value` edit atomically.
    pub fn set_key(&mut self, key: &str, value: &str) -> Result<(), UserSettingsError> {
        let old = self.clone();
        let result = if let Some(feel_key) = key.strip_prefix("feel.") {
            self.feel
                .set_key(feel_key, value)
                .map_err(UserSettingsError::Feel)
        } else if let Some(dwt_key) = key.strip_prefix("dwt.") {
            match dwt_key {
                "enabled" => {
                    self.dwt.enabled = parse_bool(value)?;
                    Ok(())
                }
                "short-timeout-ms" => {
                    self.dwt.short_timeout_ms = parse_u64(value)?;
                    Ok(())
                }
                "long-timeout-ms" => {
                    self.dwt.long_timeout_ms = parse_u64(value)?;
                    Ok(())
                }
                _ => Err(UserSettingsError::UnknownKey(key.to_string())),
            }
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
    #[error("invalid DWT settings: {0}")]
    Dwt(DwtConfigError),
    #[error("invalid gesture settings: {0}")]
    Gestures(GestureMapError),
    #[error("unknown settings key {0:?}")]
    UnknownKey(String),
    #[error("unknown gesture trigger {0:?}")]
    UnknownGesture(String),
    #[error("unknown gesture target {0:?}")]
    UnknownTarget(String),
}

fn parse_bool(value: &str) -> Result<bool, UserSettingsError> {
    value
        .parse::<bool>()
        .map_err(|_| UserSettingsError::UnknownTarget(value.to_string()))
}

fn parse_u64(value: &str) -> Result<u64, UserSettingsError> {
    value
        .parse::<u64>()
        .map_err(|_| UserSettingsError::UnknownTarget(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_v1_json_without_dwt_receives_defaults() {
        let value = serde_json::json!({
            "version": 1,
            "feel": FeelConfig::default(),
            "gestures": GestureMapConfig::default()
        });
        let settings: UserSettings = serde_json::from_value(value).unwrap();
        assert_eq!(settings.dwt, DwtConfig::default());
        settings.validate().unwrap();
    }

    #[test]
    fn dwt_keys_are_editable_and_invalid_edits_roll_back() {
        let mut settings = UserSettings::default();
        settings.set_key("dwt.enabled", "false").unwrap();
        settings.set_key("dwt.short-timeout-ms", "150").unwrap();
        settings.set_key("dwt.long-timeout-ms", "450").unwrap();
        assert!(!settings.dwt.enabled);
        assert_eq!(settings.dwt.short_timeout_ms, 150);
        assert_eq!(settings.dwt.long_timeout_ms, 450);

        let before = settings.clone();
        assert!(settings.set_key("dwt.short-timeout-ms", "900").is_err());
        assert_eq!(settings, before);
    }
}
