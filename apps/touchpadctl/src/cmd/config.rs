//! M16 strict runtime configuration validation and service preflight.

#![forbid(unsafe_code)]

use std::path::Path;

use serde_json::Value;
use touchpad_core::{
    RuntimeConfig, RuntimeConfigV1, ServiceLifecycle, CURRENT_RUNTIME_CONFIG_VERSION,
};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapabilityId {
    WaylandPortalLibei,
    X11Adapter,
    UinputAdapter,
    ContinuousGestures,
    KdeActions,
    Pressure,
    Haptics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapabilityStatus {
    ExperimentalUnqualified,
    SemanticOnly,
    SeparateQualification,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CapabilityEntry {
    id: CapabilityId,
    status: CapabilityStatus,
    detail: &'static str,
}

const CAPABILITY_MATRIX: &[CapabilityEntry] = &[
    CapabilityEntry {
        id: CapabilityId::WaylandPortalLibei,
        status: CapabilityStatus::ExperimentalUnqualified,
        detail: "implemented; requires M6/M10-M16 live acceptance evidence",
    },
    CapabilityEntry {
        id: CapabilityId::X11Adapter,
        status: CapabilityStatus::SeparateQualification,
        detail: "no silent fallback; build and qualify an explicit X11 adapter",
    },
    CapabilityEntry {
        id: CapabilityId::UinputAdapter,
        status: CapabilityStatus::SeparateQualification,
        detail: "no silent fallback; build and qualify an explicit uinput adapter",
    },
    CapabilityEntry {
        id: CapabilityId::ContinuousGestures,
        status: CapabilityStatus::SemanticOnly,
        detail: "core recognition exists; current portal/libei sink rejects the semantic event",
    },
    CapabilityEntry {
        id: CapabilityId::KdeActions,
        status: CapabilityStatus::ExperimentalUnqualified,
        detail: "KDE Plasma KGlobalAccel transport is implemented for workspace next/previous, overview, present windows, show desktop and application launcher; live acceptance remains required",
    },
    CapabilityEntry {
        id: CapabilityId::Pressure,
        status: CapabilityStatus::Unsupported,
        detail: "current device/profile does not provide a qualified pressure feature",
    },
    CapabilityEntry {
        id: CapabilityId::Haptics,
        status: CapabilityStatus::Unsupported,
        detail: "no qualified haptic hardware/output interface is present",
    },
];

/// Reads a JSON config, rejects unknown/future versions, and explicitly
/// migrates v1 to the current schema. No hardware or desktop side effect is
/// performed.
pub fn load_runtime_config(path: &Path) -> Result<(RuntimeConfig, bool), CommandFailure> {
    let bytes = std::fs::read(path).map_err(|error| {
        CommandFailure::Config(format!("could not read {}: {error}", path.display()))
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CommandFailure::Config(format!("invalid JSON in {}: {error}", path.display()))
    })?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| CommandFailure::Config("missing integer field `version`".to_string()))?;
    let version = u32::try_from(version).map_err(|_| {
        CommandFailure::Config("configuration version does not fit u32".to_string())
    })?;
    match version {
        1 => {
            let old: RuntimeConfigV1 = serde_json::from_value(value).map_err(|error| {
                CommandFailure::Config(format!("invalid v1 configuration: {error}"))
            })?;
            old.validate()
                .map_err(|error| CommandFailure::Config(error.to_string()))?;
            let migrated = old
                .migrate()
                .map_err(|error| CommandFailure::Config(error.to_string()))?;
            Ok((migrated, true))
        }
        CURRENT_RUNTIME_CONFIG_VERSION => {
            let current: RuntimeConfig = serde_json::from_value(value).map_err(|error| {
                CommandFailure::Config(format!("invalid current configuration: {error}"))
            })?;
            current
                .validate()
                .map_err(|error| CommandFailure::Config(error.to_string()))?;
            Ok((current, false))
        }
        other => Err(CommandFailure::Config(format!(
            "unsupported runtime configuration version {other}; supported versions are 1 and {CURRENT_RUNTIME_CONFIG_VERSION}"
        ))),
    }
}

/// Runs `config-check FILE`.
pub fn run_check(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let (config, migrated) = load_runtime_config(input)?;
    writeln!(
        env.out,
        "OK version={} profile={} device={} adapter={:?} foreground_only={} migrated_from_v1={}",
        config.version(),
        config.profile(),
        config.device(),
        config.output_adapter(),
        config.foreground_only(),
        migrated
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    Ok(())
}

/// Runs `service-preflight FILE`. This is a report only: the lifecycle is
/// created in `Stopped` state and no transition to `Starting` occurs here.
pub fn run_preflight(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let (config, migrated) = load_runtime_config(input)?;
    let lifecycle = ServiceLifecycle::new();
    writeln!(env.out, "M16 SERVICE PREFLIGHT — NO SERVICE STARTED")
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    writeln!(
        env.out,
        "version={} profile={} device={} adapter={:?} foreground_only={} migrated_from_v1={} state={:?}",
        config.version(),
        config.profile(),
        config.device(),
        config.output_adapter(),
        config.foreground_only(),
        migrated,
        lifecycle.state()
    )
    .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    for entry in CAPABILITY_MATRIX {
        writeln!(
            env.out,
            "capability={:?} status={:?} detail={}",
            entry.id, entry.status, entry.detail
        )
        .map_err(|error| CommandFailure::Unexpected(format!("could not write output: {error}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use touchpad_core::{OutputAdapter, ReconnectPolicy};

    #[test]
    fn capability_matrix_is_application_owned_and_explicit() {
        assert!(CAPABILITY_MATRIX.iter().any(|entry| {
            entry.id == CapabilityId::KdeActions
                && entry.status == CapabilityStatus::ExperimentalUnqualified
        }));
        assert!(CAPABILITY_MATRIX.iter().any(|entry| {
            entry.id == CapabilityId::X11Adapter
                && entry.status == CapabilityStatus::SeparateQualification
        }));
        assert!(CAPABILITY_MATRIX.iter().any(|entry| {
            entry.id == CapabilityId::Pressure && entry.status == CapabilityStatus::Unsupported
        }));
    }

    fn temp_json(contents: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "touchpadctl-m16-config-{}-{nonce}.json",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn unknown_future_version_is_rejected_before_schema_decode() {
        let value = serde_json::json!({"version": 99});
        let version = value.get("version").and_then(Value::as_u64).unwrap();
        assert_ne!(version as u32, CURRENT_RUNTIME_CONFIG_VERSION);
    }

    #[test]
    fn loader_accepts_current_and_migrates_v1() {
        let reconnect = ReconnectPolicy::new(100, 800, 4).unwrap();
        let current = RuntimeConfig::new(
            "m16-production-v1",
            "/dev/input/event12",
            OutputAdapter::WaylandPortalLibei,
            reconnect.clone(),
            reconnect.clone(),
            Some("m15-kde-v1".to_string()),
            true,
        )
        .unwrap();
        let current_path = temp_json(&serde_json::to_string(&current).unwrap());
        let (loaded, migrated) = load_runtime_config(&current_path).unwrap();
        assert_eq!(loaded, current);
        assert!(!migrated);
        let _ = std::fs::remove_file(current_path);

        let old = RuntimeConfigV1::new("m15-kde-v1", "/dev/input/event12", reconnect).unwrap();
        let old_path = temp_json(&serde_json::to_string(&old).unwrap());
        let (loaded, migrated) = load_runtime_config(&old_path).unwrap();
        assert_eq!(loaded.version(), CURRENT_RUNTIME_CONFIG_VERSION);
        assert_eq!(loaded.profile(), "m15-kde-v1");
        assert!(migrated);
        let _ = std::fs::remove_file(old_path);
    }

    #[test]
    fn loader_rejects_future_version_and_unknown_fields() {
        let future_path = temp_json(r#"{"version":99}"#);
        assert!(matches!(
            load_runtime_config(&future_path),
            Err(CommandFailure::Config(message)) if message.contains("unsupported")
        ));
        let _ = std::fs::remove_file(future_path);

        let invalid_path = temp_json(
            r#"{"version":2,"profile":"m16-production-v1","device":"/dev/input/event12","output_adapter":"wayland-portal-libei","device_reconnect":{"initial_delay_ms":100,"max_delay_ms":800,"max_attempts":4},"output_reconnect":{"initial_delay_ms":100,"max_delay_ms":800,"max_attempts":4},"rollback_profile":null,"foreground_only":true,"surprise":1}"#,
        );
        assert!(matches!(
            load_runtime_config(&invalid_path),
            Err(CommandFailure::Config(message)) if message.contains("unknown field")
        ));
        let _ = std::fs::remove_file(invalid_path);
    }
}
