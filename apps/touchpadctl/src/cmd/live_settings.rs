//! M19 deterministic settings-file hot reload.

#![forbid(unsafe_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use touchpad_core::{ArbiterConfig, M19Profile, OutputSink, UserSettings};
use touchpad_linux::TakeoverBridge;

use crate::exit::CommandFailure;

/// Result of one M19 settings-file poll.
#[derive(Clone, Debug, PartialEq)]
pub enum ReloadPoll {
    /// File bytes are unchanged from the previous poll.
    Unchanged,
    /// A changed file decoded and validated into a complete new config.
    Loaded {
        /// Fully validated replacement arbiter configuration.
        config: Box<ArbiterConfig>,
        /// Monotonic successful-reload generation number.
        generation: u64,
    },
    /// Changed bytes were unreadable, malformed, or invalid; runtime keeps
    /// the prior last-good configuration.
    Rejected(String),
}

/// Foreground M19 last-good settings watcher with a single latest pending
/// neutral-boundary configuration.
#[derive(Clone, Debug)]
pub struct SettingsWatcher {
    path: PathBuf,
    last_hash: u64,
    generation: u64,
    pending: Option<(ArbiterConfig, u64)>,
}

impl SettingsWatcher {
    /// Creates a watcher after startup has already validated the file.
    pub fn new(path: &Path) -> Result<Self, CommandFailure> {
        let bytes = std::fs::read(path).map_err(|error| {
            CommandFailure::Config(format!(
                "could not read watched settings {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            last_hash: hash_bytes(&bytes),
            generation: 0,
            pending: None,
        })
    }

    /// Polls once. Invalid changed content is rejected while the runtime keeps
    /// the previous last-good configuration.
    #[must_use]
    pub fn poll(&mut self) -> ReloadPoll {
        self.poll_validated(|_| Ok(()))
    }

    /// Polls once and applies an additional platform/output validator to a
    /// fully decoded [`UserSettings`] document before building the replacement
    /// Arbiter configuration. M19 real KDE uses this to keep newly edited
    /// unsupported desktop-action/continuous-gesture routes out of the live
    /// session while preserving last-good behavior.
    #[must_use]
    pub fn poll_validated<F>(&mut self, validator: F) -> ReloadPoll
    where
        F: FnOnce(&UserSettings) -> Result<(), String>,
    {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) => return ReloadPoll::Rejected(format!("read failed: {error}")),
        };
        let hash = hash_bytes(&bytes);
        if hash == self.last_hash {
            return ReloadPoll::Unchanged;
        }
        self.last_hash = hash;
        let settings: UserSettings = match serde_json::from_slice(&bytes) {
            Ok(settings) => settings,
            Err(error) => return ReloadPoll::Rejected(format!("invalid JSON: {error}")),
        };
        if let Err(error) = settings.validate() {
            return ReloadPoll::Rejected(error.to_string());
        }
        if let Err(error) = validator(&settings) {
            return ReloadPoll::Rejected(error);
        }
        let profile = match M19Profile::new(settings) {
            Ok(profile) => profile,
            Err(error) => return ReloadPoll::Rejected(error.to_string()),
        };
        let config = match profile.arbiter_config() {
            Ok(config) => config,
            Err(error) => return ReloadPoll::Rejected(error.to_string()),
        };
        self.generation = self.generation.saturating_add(1);
        ReloadPoll::Loaded {
            config: Box::new(config),
            generation: self.generation,
        }
    }

    /// Keeps only the newest valid update while an interaction is active.
    pub fn queue(&mut self, config: ArbiterConfig, generation: u64) {
        self.pending = Some((config, generation));
    }

    /// Applies the newest pending configuration at a neutral boundary.
    pub fn try_apply_pending<S: OutputSink>(
        &mut self,
        bridge: &mut TakeoverBridge<S>,
    ) -> Option<u64> {
        let (config, generation) = self.pending.take()?;
        if bridge.try_replace_config(config.clone()) {
            Some(generation)
        } else {
            self.pending = Some((config, generation));
            None
        }
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("touchpad-m19-watch-{nonce}.json"))
    }

    #[test]
    fn invalid_reload_is_rejected_then_later_valid_save_recovers() {
        let path = temp();
        std::fs::write(&path, serde_json::to_vec(&UserSettings::default()).unwrap()).unwrap();
        let mut watcher = SettingsWatcher::new(&path).unwrap();
        assert_eq!(watcher.poll(), ReloadPoll::Unchanged);
        std::fs::write(&path, b"{").unwrap();
        assert!(matches!(watcher.poll(), ReloadPoll::Rejected(_)));
        let mut settings = UserSettings::default();
        settings
            .set_key("feel.pointer.tracking_speed", "1.5")
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(matches!(
            watcher.poll(),
            ReloadPoll::Loaded { generation: 1, .. }
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pending_reload_is_latest_wins() {
        let path = temp();
        std::fs::write(&path, serde_json::to_vec(&UserSettings::default()).unwrap()).unwrap();
        let mut watcher = SettingsWatcher::new(&path).unwrap();
        let first = M19Profile::new(UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut second_settings = UserSettings::default();
        second_settings
            .set_key("feel.pointer.tracking_speed", "1.75")
            .unwrap();
        let second = M19Profile::new(second_settings)
            .unwrap()
            .arbiter_config()
            .unwrap();
        watcher.queue(first, 1);
        watcher.queue(second, 2);
        let (pending, generation) = watcher.pending.as_ref().expect("pending config");
        assert_eq!(*generation, 2);
        assert_eq!(
            pending
                .fidelity_config()
                .expect("M19 fidelity")
                .tracking_speed(),
            1.75
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn real_kde_validator_rejects_unsupported_reload_then_recovers() {
        let path = temp();
        let initial = UserSettings::macos_inspired();
        std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
        let mut watcher = SettingsWatcher::new(&path).unwrap();

        let mut unsupported = initial.clone();
        unsupported
            .set_key("gesture.edge-swipe-left", "notification-center")
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(matches!(
            watcher.poll_validated(|settings| {
                touchpad_desktop::required_real_kde_actions(&settings.gestures)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }),
            ReloadPoll::Rejected(_)
        ));

        let mut recovered = initial;
        recovered
            .set_key("feel.pointer.tracking_speed", "1.25")
            .unwrap();
        std::fs::write(&path, serde_json::to_vec(&recovered).unwrap()).unwrap();
        assert!(matches!(
            watcher.poll_validated(|settings| {
                touchpad_desktop::required_real_kde_actions(&settings.gestures)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }),
            ReloadPoll::Loaded { generation: 1, .. }
        ));
        let _ = std::fs::remove_file(path);
    }
}
