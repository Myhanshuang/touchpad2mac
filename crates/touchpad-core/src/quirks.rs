//! Versioned hardware quirk database.
//!
//! Quirks are data, not scattered product-name conditionals. The built-in
//! database is compiled into the binary for deterministic startup; the same
//! strict JSON schema is public so downstream packaging/tests can validate
//! proposed hardware entries before merging them.

#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{AxisId, DeviceProfile, DeviceQuirk};

/// Current hardware quirk database schema.
pub const QUIRK_DATABASE_VERSION: u32 = 1;

/// One explicit axis-resolution override.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisResolutionQuirk {
    /// Platform-neutral axis id assigned by the platform layer.
    pub axis_id: u32,
    /// Raw units per millimeter.
    pub units_per_mm: u32,
}

/// Match predicates for one hardware entry. Every populated predicate must
/// match; omitted predicates are wildcards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuirkMatch {
    /// Optional exact vendor id.
    pub vendor_id: Option<u16>,
    /// Optional exact product id.
    pub product_id: Option<u16>,
    /// Optional ASCII-case-insensitive substring of the kernel device name.
    pub name_contains: Option<String>,
}

impl QuirkMatch {
    fn matches(&self, name: &str, vendor_id: u16, product_id: u16) -> bool {
        self.vendor_id.is_none_or(|value| value == vendor_id)
            && self.product_id.is_none_or(|value| value == product_id)
            && self.name_contains.as_ref().is_none_or(|needle| {
                name.to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
    }
}

/// One named hardware quirk entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuirkEntry {
    /// Stable, human-readable entry name.
    pub name: String,
    /// Match predicates.
    #[serde(rename = "match")]
    pub matcher: QuirkMatch,
    /// Structural/behavioral quirks.
    #[serde(default)]
    pub quirks: Vec<DeviceQuirk>,
    /// Explicit axis-resolution corrections.
    #[serde(default)]
    pub axis_resolutions: Vec<AxisResolutionQuirk>,
}

/// Strict versioned quirk database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuirkDatabase {
    /// Schema version.
    pub version: u32,
    /// Ordered entries; first match wins so specific entries should precede
    /// generic ones.
    pub entries: Vec<QuirkEntry>,
}

impl QuirkDatabase {
    /// Parses and validates one strict JSON database.
    pub fn parse_json(bytes: &[u8]) -> Result<Self, QuirkDatabaseError> {
        let database: Self = serde_json::from_slice(bytes)
            .map_err(|error| QuirkDatabaseError::Json(error.to_string()))?;
        database.validate()?;
        Ok(database)
    }

    /// Validates schema/version and semantic invariants.
    pub fn validate(&self) -> Result<(), QuirkDatabaseError> {
        if self.version != QUIRK_DATABASE_VERSION {
            return Err(QuirkDatabaseError::UnsupportedVersion(self.version));
        }
        for entry in &self.entries {
            if entry.name.trim().is_empty() {
                return Err(QuirkDatabaseError::EmptyName);
            }
            if entry.matcher.vendor_id.is_none()
                && entry.matcher.product_id.is_none()
                && entry.matcher.name_contains.is_none()
            {
                return Err(QuirkDatabaseError::EmptyMatch(entry.name.clone()));
            }
            for resolution in &entry.axis_resolutions {
                if resolution.units_per_mm == 0 {
                    return Err(QuirkDatabaseError::ZeroResolution {
                        entry: entry.name.clone(),
                        axis_id: resolution.axis_id,
                    });
                }
            }
        }
        Ok(())
    }

    /// Resolves a device into a profile. Unknown hardware gets `default`.
    #[must_use]
    pub fn profile_for(&self, name: &str, vendor_id: u16, product_id: u16) -> DeviceProfile {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.matcher.matches(name, vendor_id, product_id))
        else {
            return DeviceProfile::new("default");
        };
        let mut profile = DeviceProfile::new(entry.name.clone());
        for quirk in &entry.quirks {
            profile = profile.with_quirk(*quirk);
        }
        for resolution in &entry.axis_resolutions {
            if let Some(value) = NonZeroU32::new(resolution.units_per_mm) {
                profile = profile.with_axis_resolution(AxisId::new(resolution.axis_id), value);
            }
        }
        profile
    }
}

/// Returns the validated database compiled into this release.
#[must_use]
pub fn builtin_quirks() -> QuirkDatabase {
    QuirkDatabase::parse_json(include_bytes!("../../../quirks/builtin.json"))
        .expect("repository built-in quirk database must validate in CI")
}

/// Quirk database validation error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuirkDatabaseError {
    /// JSON/schema decoding failed.
    #[error("invalid quirk database JSON: {0}")]
    Json(String),
    /// Unsupported schema version.
    #[error("unsupported quirk database version {0}")]
    UnsupportedVersion(u32),
    /// Entry name is empty.
    #[error("quirk entry name must not be empty")]
    EmptyName,
    /// Entry contains no match predicate.
    #[error("quirk entry {0:?} must contain at least one match predicate")]
    EmptyMatch(String),
    /// Resolution override is zero.
    #[error("quirk entry {entry:?} has zero resolution for axis {axis_id}")]
    ZeroResolution {
        /// Entry name.
        entry: String,
        /// Axis id.
        axis_id: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_database_is_strict_and_matches_known_device() {
        let database = builtin_quirks();
        database.validate().unwrap();
        let profile = database.profile_for("CIRQ1080:00 0488:1054 Touchpad", 0x0488, 0x1054);
        assert_eq!(profile.name, "cirq1080-0488-1054");
        assert_eq!(profile.quirks, vec![DeviceQuirk::Buttonpad]);
    }

    #[test]
    fn unknown_hardware_uses_default_profile() {
        assert_eq!(
            builtin_quirks().profile_for("Unknown", 1, 2).name,
            "default"
        );
    }

    #[test]
    fn malformed_database_fails_closed() {
        let error = QuirkDatabase::parse_json(br#"{"version":1,"entries":[{"name":"bad","match":{"vendor_id":null,"product_id":null,"name_contains":null},"quirks":[],"axis_resolutions":[]}]}"#)
            .unwrap_err();
        assert!(matches!(error, QuirkDatabaseError::EmptyMatch(_)));
    }
}
