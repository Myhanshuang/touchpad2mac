//! M16 productionization contracts: versioned runtime configuration,
//! reconnect policy, foreground service lifecycle, and capability reporting.
//!
//! The module is intentionally platform-neutral and performs no I/O. It does
//! not install a service, open a device, start a portal session, or select an
//! adapter. Callers may use these types to validate configuration and drive
//! an explicitly user-started foreground service.

#![forbid(unsafe_code)]

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The current strict runtime configuration schema version.
pub const CURRENT_RUNTIME_CONFIG_VERSION: u32 = 2;

/// The original M16 runtime configuration schema. It is retained only as an
/// explicit migration input; new configuration must use [`RuntimeConfig`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigV1 {
    version: u32,
    profile: String,
    device: String,
    reconnect: ReconnectPolicy,
}

impl RuntimeConfigV1 {
    /// Constructs a validated v1 configuration for migration tests/tools.
    pub fn new(
        profile: impl Into<String>,
        device: impl Into<String>,
        reconnect: ReconnectPolicy,
    ) -> Result<Self, RuntimeConfigError> {
        let value = Self {
            version: 1,
            profile: profile.into(),
            device: device.into(),
            reconnect,
        };
        value.validate()?;
        Ok(value)
    }

    /// Schema version encoded by this document.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Selected policy profile name.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Explicit input device path/identifier.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Shared v1 reconnect policy.
    #[must_use]
    pub const fn reconnect(&self) -> &ReconnectPolicy {
        &self.reconnect
    }

    /// Validates a decoded v1 document before migration.
    pub fn validate(&self) -> Result<(), RuntimeConfigError> {
        if self.version != 1 {
            return Err(RuntimeConfigError::UnsupportedVersion(self.version));
        }
        validate_profile_name(&self.profile)?;
        validate_nonempty("device", &self.device)?;
        self.reconnect.validate()?;
        Ok(())
    }

    /// Migrates v1 to the current schema. The single v1 reconnect policy is
    /// copied to both device and output controllers; the current supported
    /// adapter is made explicit and foreground-only operation is enforced.
    pub fn migrate(self) -> Result<RuntimeConfig, RuntimeConfigError> {
        self.validate()?;
        RuntimeConfig::new(
            self.profile,
            self.device,
            OutputAdapter::WaylandPortalLibei,
            self.reconnect.clone(),
            self.reconnect,
            None,
            true,
        )
    }
}

/// Explicit output adapter selected by the current runtime schema.
///
/// M16 deliberately exposes only the already-built Wayland portal/libei
/// adapter. X11 and uinput are listed in the capability matrix as requiring
/// separate qualification and are never selected as silent fallbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputAdapter {
    /// XDG RemoteDesktop portal + libei sender.
    WaylandPortalLibei,
}

/// Current strict runtime configuration (schema v2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    version: u32,
    profile: String,
    device: String,
    output_adapter: OutputAdapter,
    device_reconnect: ReconnectPolicy,
    output_reconnect: ReconnectPolicy,
    rollback_profile: Option<String>,
    foreground_only: bool,
}

impl RuntimeConfig {
    /// Constructs and validates a current runtime configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile: impl Into<String>,
        device: impl Into<String>,
        output_adapter: OutputAdapter,
        device_reconnect: ReconnectPolicy,
        output_reconnect: ReconnectPolicy,
        rollback_profile: Option<String>,
        foreground_only: bool,
    ) -> Result<Self, RuntimeConfigError> {
        let value = Self {
            version: CURRENT_RUNTIME_CONFIG_VERSION,
            profile: profile.into(),
            device: device.into(),
            output_adapter,
            device_reconnect,
            output_reconnect,
            rollback_profile,
            foreground_only,
        };
        value.validate()?;
        Ok(value)
    }

    /// Current schema version.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Explicit selected policy profile.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Explicit input device path/identifier.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Selected output adapter.
    #[must_use]
    pub const fn output_adapter(&self) -> OutputAdapter {
        self.output_adapter
    }

    /// Device reconnect policy.
    #[must_use]
    pub const fn device_reconnect(&self) -> &ReconnectPolicy {
        &self.device_reconnect
    }

    /// Output-session reconnect policy.
    #[must_use]
    pub const fn output_reconnect(&self) -> &ReconnectPolicy {
        &self.output_reconnect
    }

    /// Optional explicitly validated rollback profile.
    #[must_use]
    pub fn rollback_profile(&self) -> Option<&str> {
        self.rollback_profile.as_deref()
    }

    /// M16 only permits explicit foreground service operation.
    #[must_use]
    pub const fn foreground_only(&self) -> bool {
        self.foreground_only
    }

    /// Strict semantic validation after deserialization.
    pub fn validate(&self) -> Result<(), RuntimeConfigError> {
        if self.version != CURRENT_RUNTIME_CONFIG_VERSION {
            return Err(RuntimeConfigError::UnsupportedVersion(self.version));
        }
        validate_profile_name(&self.profile)?;
        validate_nonempty("device", &self.device)?;
        self.device_reconnect.validate()?;
        self.output_reconnect.validate()?;
        if let Some(profile) = &self.rollback_profile {
            validate_profile_name(profile)?;
            if profile == &self.profile {
                return Err(RuntimeConfigError::RollbackMatchesActive);
            }
        }
        if !self.foreground_only {
            return Err(RuntimeConfigError::PersistentServiceNotQualified);
        }
        Ok(())
    }
}

fn validate_nonempty(field: &'static str, value: &str) -> Result<(), RuntimeConfigError> {
    if value.trim().is_empty() {
        Err(RuntimeConfigError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn validate_profile_name(profile: &str) -> Result<(), RuntimeConfigError> {
    let valid = matches!(
        profile,
        crate::m10::M10_LINEAR_V1_NAME
            | crate::m11::M11_FIDELITY_V1_NAME
            | crate::m12::M12_SCROLL_V1_NAME
            | crate::m13::M13_ROBUST_V1_NAME
            | crate::m14::M14_GESTURES_V1_NAME
            | crate::m15::M15_KDE_V1_NAME
            | crate::m16::M16_PRODUCTION_V1_NAME
    );
    if valid {
        Ok(())
    } else {
        Err(RuntimeConfigError::UnknownProfile(profile.to_string()))
    }
}

/// Strict runtime configuration validation/migration errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeConfigError {
    /// The decoded schema version is not supported by this binary.
    #[error("unsupported runtime configuration version {0}")]
    UnsupportedVersion(u32),
    /// A required textual field is empty.
    #[error("runtime configuration field {0} must not be empty")]
    EmptyField(&'static str),
    /// The requested profile is not one of the versioned built-in profiles.
    #[error("unknown runtime policy profile {0:?}")]
    UnknownProfile(String),
    /// Active and rollback profiles must be distinct.
    #[error("rollback_profile must differ from the active profile")]
    RollbackMatchesActive,
    /// Persistent/autostart mode is intentionally unavailable in M16.
    #[error("persistent service enablement is not qualified; foreground_only must be true")]
    PersistentServiceNotQualified,
    /// One reconnect policy is invalid.
    #[error(transparent)]
    Reconnect(#[from] ReconnectPolicyError),
}

/// Serializable bounded exponential reconnect policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconnectPolicy {
    initial_delay_ms: u64,
    max_delay_ms: u64,
    max_attempts: u32,
}

impl ReconnectPolicy {
    /// Constructs a validated reconnect policy.
    pub fn new(
        initial_delay_ms: u64,
        max_delay_ms: u64,
        max_attempts: u32,
    ) -> Result<Self, ReconnectPolicyError> {
        let value = Self {
            initial_delay_ms,
            max_delay_ms,
            max_attempts,
        };
        value.validate()?;
        Ok(value)
    }

    /// Initial retry delay.
    #[must_use]
    pub const fn initial_delay_ms(&self) -> u64 {
        self.initial_delay_ms
    }

    /// Maximum retry delay.
    #[must_use]
    pub const fn max_delay_ms(&self) -> u64 {
        self.max_delay_ms
    }

    /// Maximum failures that may schedule a retry.
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Validates the bounded-backoff invariants.
    pub fn validate(&self) -> Result<(), ReconnectPolicyError> {
        if self.initial_delay_ms == 0 {
            return Err(ReconnectPolicyError::ZeroInitialDelay);
        }
        if self.max_delay_ms < self.initial_delay_ms {
            return Err(ReconnectPolicyError::MaxBelowInitial);
        }
        if self.max_attempts == 0 {
            return Err(ReconnectPolicyError::ZeroAttempts);
        }
        if self.max_attempts > 32 {
            return Err(ReconnectPolicyError::TooManyAttempts(self.max_attempts));
        }
        Ok(())
    }
}

/// Reconnect policy validation errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReconnectPolicyError {
    /// Retry delay cannot be zero.
    #[error("reconnect initial_delay_ms must be greater than zero")]
    ZeroInitialDelay,
    /// Maximum delay must not be below the initial delay.
    #[error("reconnect max_delay_ms must be >= initial_delay_ms")]
    MaxBelowInitial,
    /// A reconnect controller must permit at least one attempt.
    #[error("reconnect max_attempts must be greater than zero")]
    ZeroAttempts,
    /// Retry count is deliberately bounded to prevent effectively unbounded
    /// automatic recovery loops.
    #[error("reconnect max_attempts {0} exceeds the supported limit 32")]
    TooManyAttempts(u32),
}

/// Result of one reconnect-controller failure transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectDecision {
    /// Retry after the deterministic bounded delay.
    RetryAfter(Duration),
    /// Retry cap was reached; caller must surface a degraded/faulted state.
    Exhausted,
    /// Controller was explicitly stopped; no more retries may be scheduled.
    Stopped,
}

/// Deterministic bounded exponential reconnect controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconnectController {
    policy: ReconnectPolicy,
    attempts: u32,
    next_delay_ms: u64,
    stopped: bool,
}

impl ReconnectController {
    /// Creates an active controller with zero attempts consumed.
    #[must_use]
    pub fn new(policy: ReconnectPolicy) -> Self {
        let next_delay_ms = policy.initial_delay_ms;
        Self {
            policy,
            attempts: 0,
            next_delay_ms,
            stopped: false,
        }
    }

    /// Number of scheduled retries since the last success/reset.
    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Whether explicit stop/cancel has disabled future retries.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Records one failure and returns the next action.
    pub fn on_failure(&mut self) -> ReconnectDecision {
        if self.stopped {
            return ReconnectDecision::Stopped;
        }
        if self.attempts >= self.policy.max_attempts {
            return ReconnectDecision::Exhausted;
        }
        let delay = self.next_delay_ms.min(self.policy.max_delay_ms);
        self.attempts += 1;
        self.next_delay_ms = self
            .next_delay_ms
            .saturating_mul(2)
            .min(self.policy.max_delay_ms);
        ReconnectDecision::RetryAfter(Duration::from_millis(delay))
    }

    /// Successful reconnection resets the retry budget/backoff.
    pub fn on_success(&mut self) {
        if !self.stopped {
            self.attempts = 0;
            self.next_delay_ms = self.policy.initial_delay_ms;
        }
    }

    /// Explicit stop/cancel is sticky and idempotent.
    pub fn stop(&mut self) {
        self.stopped = true;
    }
}

/// Foreground service lifecycle states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceState {
    /// No resources are owned.
    #[default]
    Stopped,
    /// Configuration validated; resources are being prepared.
    Starting,
    /// Device and output are operating normally.
    Running,
    /// A bounded device/output reconnect sequence is active.
    Reconnecting,
    /// Service remains alive but one capability is intentionally unavailable.
    Degraded,
    /// Ordered idempotent shutdown is in progress.
    Stopping,
    /// Recovery failed or a non-recoverable invariant was hit.
    Faulted,
}

/// Explicit service lifecycle transition error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("illegal service transition {from:?} -> {to:?}")]
pub struct ServiceTransitionError {
    /// Previous lifecycle state.
    pub from: ServiceState,
    /// Requested next lifecycle state.
    pub to: ServiceState,
}

/// Platform-neutral foreground service lifecycle controller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServiceLifecycle {
    state: ServiceState,
}

impl ServiceLifecycle {
    /// Creates a stopped lifecycle.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ServiceState::Stopped,
        }
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> ServiceState {
        self.state
    }

    /// Performs one explicitly allowed lifecycle transition.
    pub fn transition(&mut self, to: ServiceState) -> Result<(), ServiceTransitionError> {
        let from = self.state;
        let legal = matches!(
            (from, to),
            (ServiceState::Stopped, ServiceState::Starting)
                | (ServiceState::Starting, ServiceState::Running)
                | (ServiceState::Starting, ServiceState::Degraded)
                | (ServiceState::Starting, ServiceState::Faulted)
                | (ServiceState::Starting, ServiceState::Stopping)
                | (ServiceState::Running, ServiceState::Reconnecting)
                | (ServiceState::Running, ServiceState::Degraded)
                | (ServiceState::Running, ServiceState::Faulted)
                | (ServiceState::Running, ServiceState::Stopping)
                | (ServiceState::Reconnecting, ServiceState::Running)
                | (ServiceState::Reconnecting, ServiceState::Degraded)
                | (ServiceState::Reconnecting, ServiceState::Faulted)
                | (ServiceState::Reconnecting, ServiceState::Stopping)
                | (ServiceState::Degraded, ServiceState::Running)
                | (ServiceState::Degraded, ServiceState::Reconnecting)
                | (ServiceState::Degraded, ServiceState::Faulted)
                | (ServiceState::Degraded, ServiceState::Stopping)
                | (ServiceState::Faulted, ServiceState::Starting)
                | (ServiceState::Faulted, ServiceState::Stopping)
                | (ServiceState::Stopping, ServiceState::Stopped)
        );
        if !legal {
            return Err(ServiceTransitionError { from, to });
        }
        self.state = to;
        Ok(())
    }

    /// Requests ordered shutdown. Repeated requests while stopping/stopped
    /// are no-ops, making the shutdown entry point idempotent.
    pub fn request_stop(&mut self) -> Result<(), ServiceTransitionError> {
        match self.state {
            ServiceState::Stopped | ServiceState::Stopping => Ok(()),
            _ => self.transition(ServiceState::Stopping),
        }
    }

    /// Acknowledges completion of ordered cleanup. Repeated acknowledgement
    /// after already stopped is also idempotent.
    pub fn mark_stopped(&mut self) -> Result<(), ServiceTransitionError> {
        match self.state {
            ServiceState::Stopped => Ok(()),
            ServiceState::Stopping => self.transition(ServiceState::Stopped),
            other => Err(ServiceTransitionError {
                from: other,
                to: ServiceState::Stopped,
            }),
        }
    }
}

/// Capability/adapter rows reported by M16 preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityId {
    /// Existing Wayland portal + libei output adapter.
    WaylandPortalLibei,
    /// Future X11 output/input adapter.
    X11Adapter,
    /// Future uinput output adapter.
    UinputAdapter,
    /// Continuous gesture semantic output from M14.
    ContinuousGestures,
    /// KDE semantic action transport from M15.
    KdeActions,
    /// Pressure-sensitive interactions.
    Pressure,
    /// Haptic output.
    Haptics,
}

/// Qualification status for a capability row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityStatus {
    /// Implemented but still requires the documented user-run live evidence.
    ExperimentalUnqualified,
    /// Exists only as a semantic/injected adapter boundary; no real transport
    /// is enabled by the current stack.
    SemanticOnly,
    /// A future adapter must be built and independently qualified.
    SeparateQualification,
    /// Current hardware/output stack does not provide the capability.
    Unsupported,
}

/// One immutable capability matrix entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityEntry {
    /// Capability/adapter identifier.
    pub id: CapabilityId,
    /// Current qualification status.
    pub status: CapabilityStatus,
    /// Human-readable boundary/evidence statement.
    pub detail: &'static str,
}

/// Current conservative M16 capability matrix.
#[must_use]
pub const fn capability_matrix() -> &'static [CapabilityEntry] {
    &[
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
            detail:
                "core recognition exists; current M6 portal/libei sink rejects the semantic event",
        },
        CapabilityEntry {
            id: CapabilityId::KdeActions,
            status: CapabilityStatus::ExperimentalUnqualified,
            detail: "real M19 KDE Plasma KGlobalAccel transport is implemented for workspace next/previous, overview, present windows, show desktop and application launcher; live acceptance remains required",
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconnect() -> ReconnectPolicy {
        ReconnectPolicy::new(100, 800, 4).unwrap()
    }

    #[test]
    fn v1_migrates_to_current_without_silent_runtime_enablement() {
        let old = RuntimeConfigV1::new("m15-kde-v1", "/dev/input/event12", reconnect()).unwrap();
        let current = old.migrate().unwrap();
        assert_eq!(current.version(), CURRENT_RUNTIME_CONFIG_VERSION);
        assert_eq!(current.profile(), "m15-kde-v1");
        assert_eq!(current.output_adapter(), OutputAdapter::WaylandPortalLibei);
        assert_eq!(current.device_reconnect(), current.output_reconnect());
        assert!(current.foreground_only());
    }

    #[test]
    fn current_config_is_strictly_validated() {
        let cfg = RuntimeConfig::new(
            "m16-production-v1",
            "/dev/input/event12",
            OutputAdapter::WaylandPortalLibei,
            reconnect(),
            reconnect(),
            Some("m15-kde-v1".to_string()),
            true,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert!(RuntimeConfig::new(
            "unknown",
            "/dev/input/event12",
            OutputAdapter::WaylandPortalLibei,
            reconnect(),
            reconnect(),
            None,
            true,
        )
        .is_err());
        assert!(RuntimeConfig::new(
            "m16-production-v1",
            "/dev/input/event12",
            OutputAdapter::WaylandPortalLibei,
            reconnect(),
            reconnect(),
            None,
            false,
        )
        .is_err());
    }

    #[test]
    fn reconnect_backoff_caps_resets_and_stops() {
        let policy = reconnect();
        let mut c = ReconnectController::new(policy);
        assert_eq!(
            c.on_failure(),
            ReconnectDecision::RetryAfter(Duration::from_millis(100))
        );
        assert_eq!(
            c.on_failure(),
            ReconnectDecision::RetryAfter(Duration::from_millis(200))
        );
        assert_eq!(
            c.on_failure(),
            ReconnectDecision::RetryAfter(Duration::from_millis(400))
        );
        assert_eq!(
            c.on_failure(),
            ReconnectDecision::RetryAfter(Duration::from_millis(800))
        );
        assert_eq!(c.on_failure(), ReconnectDecision::Exhausted);
        c.on_success();
        assert_eq!(c.attempts(), 0);
        assert_eq!(
            c.on_failure(),
            ReconnectDecision::RetryAfter(Duration::from_millis(100))
        );
        c.stop();
        c.stop();
        assert_eq!(c.on_failure(), ReconnectDecision::Stopped);
    }

    #[test]
    fn service_lifecycle_has_explicit_idempotent_shutdown() {
        let mut service = ServiceLifecycle::new();
        service.transition(ServiceState::Starting).unwrap();
        service.transition(ServiceState::Running).unwrap();
        service.transition(ServiceState::Reconnecting).unwrap();
        service.transition(ServiceState::Degraded).unwrap();
        service.request_stop().unwrap();
        service.request_stop().unwrap();
        assert_eq!(service.state(), ServiceState::Stopping);
        service.mark_stopped().unwrap();
        service.mark_stopped().unwrap();
        assert_eq!(service.state(), ServiceState::Stopped);
        assert!(service.transition(ServiceState::Running).is_err());
    }

    #[test]
    fn capability_matrix_is_explicit_about_unqualified_and_unsupported_paths() {
        let rows = capability_matrix();
        assert!(rows.iter().any(|e| {
            e.id == CapabilityId::X11Adapter && e.status == CapabilityStatus::SeparateQualification
        }));
        assert!(rows.iter().any(|e| {
            e.id == CapabilityId::UinputAdapter
                && e.status == CapabilityStatus::SeparateQualification
        }));
        assert!(rows.iter().any(|e| {
            e.id == CapabilityId::Pressure && e.status == CapabilityStatus::Unsupported
        }));
        assert!(rows.iter().any(|e| {
            e.id == CapabilityId::Haptics && e.status == CapabilityStatus::Unsupported
        }));
    }
}
