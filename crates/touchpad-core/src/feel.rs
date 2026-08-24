//! M17 strict, versioned feel-tuning overlay.
//!
//! `FeelConfig` deliberately contains only perceptual tuning controls. Device
//! selection, live takeover opt-ins, cleanup, reconnect, service lifecycle,
//! tap timing and output qualification remain owned by earlier milestones.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const FEEL_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerFeel {
    pub dead_zone_radius_mm: f64,
    pub tracking_speed: f64,
    pub min_gain: f64,
    pub max_gain: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollFeel {
    pub min_gain: f64,
    pub max_gain: f64,
    pub axis_lock_engage_ratio: f64,
    pub axis_lock_release_ratio: f64,
    pub momentum_tau_ms: u64,
    pub momentum_start_speed_mm_per_s: f64,
    pub momentum_stop_speed_mm_per_s: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureFeel {
    pub pinch_commit_mm: f64,
    pub page_swipe_commit_mm: f64,
    pub multi_swipe_commit_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DragFeel {
    pub commit_threshold_mm: f64,
    pub drag_lock: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeelConfig {
    pub version: u32,
    pub pointer: PointerFeel,
    pub scroll: ScrollFeel,
    pub gesture: GestureFeel,
    pub drag: DragFeel,
}

impl Default for FeelConfig {
    fn default() -> Self {
        Self {
            version: FEEL_CONFIG_VERSION,
            pointer: PointerFeel {
                dead_zone_radius_mm: 0.09,
                tracking_speed: 1.0,
                min_gain: 1.0,
                max_gain: 2.0,
            },
            scroll: ScrollFeel {
                min_gain: 1.0,
                max_gain: 1.75,
                axis_lock_engage_ratio: 2.5,
                axis_lock_release_ratio: 1.5,
                momentum_tau_ms: 325,
                momentum_start_speed_mm_per_s: 35.0,
                momentum_stop_speed_mm_per_s: 6.0,
            },
            gesture: GestureFeel {
                pinch_commit_mm: 0.8,
                page_swipe_commit_mm: 0.8,
                multi_swipe_commit_mm: 2.0,
            },
            drag: DragFeel {
                commit_threshold_mm: 1.0,
                drag_lock: true,
            },
        }
    }
}

impl FeelConfig {
    pub fn validate(&self) -> Result<(), FeelConfigError> {
        if self.version != FEEL_CONFIG_VERSION {
            return Err(FeelConfigError::UnsupportedVersion(self.version));
        }
        check_range(
            "pointer.dead_zone_radius_mm",
            self.pointer.dead_zone_radius_mm,
            0.01,
            0.30,
        )?;
        check_range(
            "pointer.tracking_speed",
            self.pointer.tracking_speed,
            0.25,
            4.0,
        )?;
        check_range("pointer.min_gain", self.pointer.min_gain, 0.5, 2.0)?;
        check_range("pointer.max_gain", self.pointer.max_gain, 0.5, 4.0)?;
        if self.pointer.max_gain < self.pointer.min_gain {
            return Err(FeelConfigError::Ordering(
                "pointer.max_gain must be >= pointer.min_gain",
            ));
        }
        check_range("scroll.min_gain", self.scroll.min_gain, 0.5, 2.0)?;
        check_range("scroll.max_gain", self.scroll.max_gain, 0.5, 4.0)?;
        if self.scroll.max_gain < self.scroll.min_gain {
            return Err(FeelConfigError::Ordering(
                "scroll.max_gain must be >= scroll.min_gain",
            ));
        }
        check_range(
            "scroll.axis_lock_engage_ratio",
            self.scroll.axis_lock_engage_ratio,
            1.2,
            6.0,
        )?;
        check_range(
            "scroll.axis_lock_release_ratio",
            self.scroll.axis_lock_release_ratio,
            1.05,
            4.0,
        )?;
        if self.scroll.axis_lock_release_ratio >= self.scroll.axis_lock_engage_ratio {
            return Err(FeelConfigError::Ordering(
                "scroll.axis_lock_release_ratio must be < scroll.axis_lock_engage_ratio",
            ));
        }
        if !(50..=1200).contains(&self.scroll.momentum_tau_ms) {
            return Err(FeelConfigError::IntegerRange {
                name: "scroll.momentum_tau_ms",
                value: self.scroll.momentum_tau_ms,
                min: 50,
                max: 1200,
            });
        }
        check_range(
            "scroll.momentum_start_speed_mm_per_s",
            self.scroll.momentum_start_speed_mm_per_s,
            10.0,
            200.0,
        )?;
        check_range(
            "scroll.momentum_stop_speed_mm_per_s",
            self.scroll.momentum_stop_speed_mm_per_s,
            1.0,
            50.0,
        )?;
        if self.scroll.momentum_stop_speed_mm_per_s >= self.scroll.momentum_start_speed_mm_per_s {
            return Err(FeelConfigError::Ordering("scroll.momentum_stop_speed_mm_per_s must be < scroll.momentum_start_speed_mm_per_s"));
        }
        check_range(
            "gesture.pinch_commit_mm",
            self.gesture.pinch_commit_mm,
            0.2,
            3.0,
        )?;
        check_range(
            "gesture.page_swipe_commit_mm",
            self.gesture.page_swipe_commit_mm,
            0.3,
            5.0,
        )?;
        check_range(
            "gesture.multi_swipe_commit_mm",
            self.gesture.multi_swipe_commit_mm,
            1.0,
            8.0,
        )?;
        check_range(
            "drag.commit_threshold_mm",
            self.drag.commit_threshold_mm,
            0.6,
            4.0,
        )?;
        if self.drag.commit_threshold_mm >= self.gesture.multi_swipe_commit_mm {
            return Err(FeelConfigError::Ordering("drag.commit_threshold_mm must stay below gesture.multi_swipe_commit_mm so drag wins ownership first"));
        }
        Ok(())
    }

    pub fn set_key(&mut self, key: &str, value: &str) -> Result<(), FeelConfigError> {
        let old = self.clone();
        let result = match key {
            "pointer.dead_zone_radius_mm" => {
                parse_f64(value).map(|v| self.pointer.dead_zone_radius_mm = v)
            }
            "pointer.tracking_speed" => parse_f64(value).map(|v| self.pointer.tracking_speed = v),
            "pointer.min_gain" => parse_f64(value).map(|v| self.pointer.min_gain = v),
            "pointer.max_gain" => parse_f64(value).map(|v| self.pointer.max_gain = v),
            "scroll.min_gain" => parse_f64(value).map(|v| self.scroll.min_gain = v),
            "scroll.max_gain" => parse_f64(value).map(|v| self.scroll.max_gain = v),
            "scroll.axis_lock_engage_ratio" => {
                parse_f64(value).map(|v| self.scroll.axis_lock_engage_ratio = v)
            }
            "scroll.axis_lock_release_ratio" => {
                parse_f64(value).map(|v| self.scroll.axis_lock_release_ratio = v)
            }
            "scroll.momentum_tau_ms" => value
                .parse::<u64>()
                .map_err(|_| FeelConfigError::InvalidValue {
                    key: key.to_string(),
                    value: value.to_string(),
                })
                .map(|v| self.scroll.momentum_tau_ms = v),
            "scroll.momentum_start_speed_mm_per_s" => {
                parse_f64(value).map(|v| self.scroll.momentum_start_speed_mm_per_s = v)
            }
            "scroll.momentum_stop_speed_mm_per_s" => {
                parse_f64(value).map(|v| self.scroll.momentum_stop_speed_mm_per_s = v)
            }
            "gesture.pinch_commit_mm" => parse_f64(value).map(|v| self.gesture.pinch_commit_mm = v),
            "gesture.page_swipe_commit_mm" => {
                parse_f64(value).map(|v| self.gesture.page_swipe_commit_mm = v)
            }
            "gesture.multi_swipe_commit_mm" => {
                parse_f64(value).map(|v| self.gesture.multi_swipe_commit_mm = v)
            }
            "drag.commit_threshold_mm" => {
                parse_f64(value).map(|v| self.drag.commit_threshold_mm = v)
            }
            "drag.drag_lock" => value
                .parse::<bool>()
                .map_err(|_| FeelConfigError::InvalidValue {
                    key: key.to_string(),
                    value: value.to_string(),
                })
                .map(|v| self.drag.drag_lock = v),
            _ => Err(FeelConfigError::UnknownKey(key.to_string())),
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

fn parse_f64(value: &str) -> Result<f64, FeelConfigError> {
    value
        .parse::<f64>()
        .map_err(|_| FeelConfigError::InvalidNumber(value.to_string()))
}

fn check_range(name: &'static str, value: f64, min: f64, max: f64) -> Result<(), FeelConfigError> {
    if !value.is_finite() || value < min || value > max {
        Err(FeelConfigError::Range {
            name,
            value,
            min,
            max,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FeelConfigError {
    #[error("unsupported feel configuration version {0}")]
    UnsupportedVersion(u32),
    #[error("{name}={value} is outside [{min}, {max}]")]
    Range {
        name: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },
    #[error("{name}={value} is outside [{min}, {max}]")]
    IntegerRange {
        name: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },
    #[error("invalid feel parameter ordering: {0}")]
    Ordering(&'static str),
    #[error("unknown feel parameter {0:?}")]
    UnknownKey(String),
    #[error("invalid floating-point value {0:?}")]
    InvalidNumber(String),
    #[error("invalid value {value:?} for {key}")]
    InvalidValue { key: String, value: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeelParameterSpec {
    pub key: &'static str,
    pub group: &'static str,
    pub unit: &'static str,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub effect: &'static str,
}

#[must_use]
pub const fn feel_parameter_specs() -> &'static [FeelParameterSpec] {
    &[
        FeelParameterSpec { key: "pointer.dead_zone_radius_mm", group: "Pointer", unit: "mm", min: Some(0.01), max: Some(0.30), step: Some(0.01), effect: "Higher filters more tiny hand jitter; too high feels sticky." },
        FeelParameterSpec { key: "pointer.tracking_speed", group: "Pointer", unit: "x", min: Some(0.25), max: Some(4.0), step: Some(0.05), effect: "Global pointer travel multiplier." },
        FeelParameterSpec { key: "pointer.min_gain", group: "Pointer", unit: "x", min: Some(0.5), max: Some(2.0), step: Some(0.05), effect: "Slow precision-motion gain." },
        FeelParameterSpec { key: "pointer.max_gain", group: "Pointer", unit: "x", min: Some(0.5), max: Some(4.0), step: Some(0.05), effect: "Fast-motion acceleration ceiling." },
        FeelParameterSpec { key: "scroll.min_gain", group: "Scroll", unit: "x", min: Some(0.5), max: Some(2.0), step: Some(0.05), effect: "Slow scroll sensitivity." },
        FeelParameterSpec { key: "scroll.max_gain", group: "Scroll", unit: "x", min: Some(0.5), max: Some(4.0), step: Some(0.05), effect: "Fast scroll sensitivity ceiling." },
        FeelParameterSpec { key: "scroll.axis_lock_engage_ratio", group: "Scroll", unit: "ratio", min: Some(1.2), max: Some(6.0), step: Some(0.1), effect: "Higher requires a cleaner dominant direction before locking." },
        FeelParameterSpec { key: "scroll.axis_lock_release_ratio", group: "Scroll", unit: "ratio", min: Some(1.05), max: Some(4.0), step: Some(0.05), effect: "Controls how readily an existing axis lock releases." },
        FeelParameterSpec { key: "scroll.momentum_tau_ms", group: "Scroll", unit: "ms", min: Some(50.0), max: Some(1200.0), step: Some(25.0), effect: "Higher makes inertial scrolling coast longer." },
        FeelParameterSpec { key: "scroll.momentum_start_speed_mm_per_s", group: "Scroll", unit: "mm/s", min: Some(10.0), max: Some(200.0), step: Some(5.0), effect: "Higher requires a faster release to start momentum." },
        FeelParameterSpec { key: "scroll.momentum_stop_speed_mm_per_s", group: "Scroll", unit: "mm/s", min: Some(1.0), max: Some(50.0), step: Some(1.0), effect: "Higher cuts momentum off earlier." },
        FeelParameterSpec { key: "gesture.pinch_commit_mm", group: "Gestures", unit: "mm", min: Some(0.2), max: Some(3.0), step: Some(0.1), effect: "Lower makes pinch/zoom commit sooner." },
        FeelParameterSpec { key: "gesture.page_swipe_commit_mm", group: "Gestures", unit: "mm", min: Some(0.3), max: Some(5.0), step: Some(0.1), effect: "Lower makes two-finger page swipe commit sooner." },
        FeelParameterSpec { key: "gesture.multi_swipe_commit_mm", group: "Gestures", unit: "mm", min: Some(1.0), max: Some(8.0), step: Some(0.1), effect: "Lower makes 3/4-finger swipes commit sooner." },
        FeelParameterSpec { key: "drag.commit_threshold_mm", group: "Three-finger drag", unit: "mm", min: Some(0.6), max: Some(4.0), step: Some(0.1), effect: "Lower makes three-finger drag grab sooner; must remain below multi-swipe threshold." },
        FeelParameterSpec { key: "drag.drag_lock", group: "Three-finger drag", unit: "bool", min: None, max: None, step: None, effect: "Keep the drag held after lifting three fingers until a release tap." },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_specs_are_stable() {
        FeelConfig::default().validate().unwrap();
        assert_eq!(feel_parameter_specs().len(), 16);
    }

    #[test]
    fn unsafe_or_priority_breaking_edits_are_rejected_atomically() {
        let mut config = FeelConfig::default();
        let original = config.clone();
        assert!(config.set_key("pointer.tracking_speed", "20").is_err());
        assert_eq!(config, original);
        assert!(config.set_key("drag.commit_threshold_mm", "2.5").is_err());
        assert_eq!(config, original);
        assert!(config
            .set_key("scroll.axis_lock_release_ratio", "3")
            .is_err());
        assert_eq!(config, original);
    }

    #[test]
    fn strict_json_rejects_unknown_fields_and_future_versions() {
        let json = serde_json::to_string(&FeelConfig::default()).unwrap();
        let decoded: FeelConfig = serde_json::from_str(&json).unwrap();
        decoded.validate().unwrap();
        let bad = json.replace("\"version\":1", "\"version\":2");
        let decoded: FeelConfig = serde_json::from_str(&bad).unwrap();
        assert!(matches!(
            decoded.validate(),
            Err(FeelConfigError::UnsupportedVersion(2))
        ));
        let unknown = json.replacen("{", "{\"surprise\":1,", 1);
        assert!(serde_json::from_str::<FeelConfig>(&unknown).is_err());
    }
}
