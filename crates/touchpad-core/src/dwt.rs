//! Platform-neutral disable-while-typing (DWT) policy.
//!
//! The Linux boundary is responsible for identifying keyboards and reducing
//! raw key events to an anonymous "typing activity happened" signal.  Core
//! deliberately never receives key codes, key text, or keyboard device data.

#![forbid(unsafe_code)]

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// libinput-style DWT defaults: the first qualifying key uses a short
/// timeout; continued typing extends the quiet period.
pub const DEFAULT_DWT_SHORT_TIMEOUT_MS: u64 = 200;
pub const DEFAULT_DWT_LONG_TIMEOUT_MS: u64 = 500;

/// User-facing DWT policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DwtConfig {
    /// Whether typing activity may suppress new touch contacts.
    pub enabled: bool,
    /// Quiet period after the first isolated typing key.
    pub short_timeout_ms: u64,
    /// Quiet period once continued typing has been detected.
    pub long_timeout_ms: u64,
}

impl Default for DwtConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            short_timeout_ms: DEFAULT_DWT_SHORT_TIMEOUT_MS,
            long_timeout_ms: DEFAULT_DWT_LONG_TIMEOUT_MS,
        }
    }
}

impl DwtConfig {
    /// Validates user-configurable DWT timing without silently coercing it.
    pub fn validate(&self) -> Result<(), DwtConfigError> {
        const MAX_TIMEOUT_MS: u64 = 5_000;
        if !(1..=MAX_TIMEOUT_MS).contains(&self.short_timeout_ms) {
            return Err(DwtConfigError::TimeoutRange {
                name: "short_timeout_ms",
                value: self.short_timeout_ms,
                max: MAX_TIMEOUT_MS,
            });
        }
        if !(1..=MAX_TIMEOUT_MS).contains(&self.long_timeout_ms) {
            return Err(DwtConfigError::TimeoutRange {
                name: "long_timeout_ms",
                value: self.long_timeout_ms,
                max: MAX_TIMEOUT_MS,
            });
        }
        if self.short_timeout_ms > self.long_timeout_ms {
            return Err(DwtConfigError::Ordering);
        }
        Ok(())
    }

    #[must_use]
    pub const fn short_timeout(&self) -> Duration {
        Duration::from_millis(self.short_timeout_ms)
    }

    #[must_use]
    pub const fn long_timeout(&self) -> Duration {
        Duration::from_millis(self.long_timeout_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DwtConfigError {
    #[error("DWT {name} must be in 1..={max} ms, got {value}")]
    TimeoutRange {
        name: &'static str,
        value: u64,
        max: u64,
    },
    #[error("DWT short_timeout_ms must be <= long_timeout_ms")]
    Ordering,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_libinput_short_and_long_windows() {
        let config = DwtConfig::default();
        assert_eq!(config.short_timeout(), Duration::from_millis(200));
        assert_eq!(config.long_timeout(), Duration::from_millis(500));
        assert!(config.enabled);
        config.validate().unwrap();
    }

    #[test]
    fn invalid_timeout_order_is_rejected() {
        let config = DwtConfig {
            short_timeout_ms: 600,
            ..DwtConfig::default()
        };
        assert_eq!(config.validate(), Err(DwtConfigError::Ordering));
    }
}
