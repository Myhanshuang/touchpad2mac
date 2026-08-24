#![forbid(unsafe_code)]
//! Structured errors for the desktop output adapter (M6).
//!
//! One error taxonomy covers the whole adapter — the RemoteDesktop portal
//! (D-Bus) client, the libei sender transport, the session lifecycle, the
//! bounded emit pattern and the environment probe — so every failure is an
//! honest, actionable, structured result and no path can degrade a failure
//! into a silent success. The `touchpadctl output-probe` command maps each
//! variant onto a documented stable exit code (see `apps/touchpadctl`
//! `cmd::output_probe` and the README).

/// A structured failure of the desktop output adapter.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DesktopOutputError {
    /// The D-Bus session bus is unreachable (no `DBUS_SESSION_BUS_ADDRESS`,
    /// no `$XDG_RUNTIME_DIR/bus`, the bus refused the connection, …).
    #[error("no D-Bus session bus: {0}")]
    NoSessionBus(String),

    /// The `org.freedesktop.portal.Desktop` service or the
    /// `org.freedesktop.portal.RemoteDesktop` interface is not available.
    #[error("RemoteDesktop portal unavailable: {0}")]
    PortalUnavailable(String),

    /// The portal protocol is too old to provide the required capability.
    /// On this host the RemoteDesktop interface is version 2, whose
    /// `ConnectToEIS` method returns the EIS socket fd for libei.
    #[error(
        "RemoteDesktop portal protocol version {found} cannot provide the required capability \
         (need version {required}): {detail}"
    )]
    ProtocolUnsupported {
        /// The minimum interface version the operation needs.
        required: u32,
        /// The interface version the portal actually exposes.
        found: u32,
        /// What exactly is missing.
        detail: String,
    },

    /// The user dismissed/cancelled the authorization dialog.
    #[error("authorization cancelled by the user")]
    AuthorizationCancelled,

    /// The portal refused authorization (response code 2, or a D-Bus error).
    #[error("authorization refused by the portal (response {response}): {message}")]
    AuthorizationRefused {
        /// The portal response code.
        response: u32,
        /// The portal-provided message, if any.
        message: String,
    },

    /// The libei runtime library could not be loaded.
    #[error("libei library missing: {0}")]
    LibraryMissing(String),

    /// The libei/EIS transport disconnected unexpectedly.
    #[error("transport disconnected: {0}")]
    TransportDisconnected(String),

    /// The EIS server paused the active device mid-emission; output through
    /// it is discarded until the session is released.
    #[error("the EIS device was paused by the server: {0}")]
    DevicePaused(String),

    /// A single event could not be sent to the transport (partial send
    /// failure — never reported as a success).
    #[error("could not send event: {0}")]
    SendFailed(String),

    /// Releasing held button/key/scroll state failed during shutdown.
    #[error("could not release held state: {0}")]
    ReleaseFailed(String),

    /// Preparing the session failed, and cleaning up the partially-prepared
    /// session also failed. The **primary** failure (and its category/exit
    /// precedence) is preserved as `primary`; `cleanup` carries the cleanup
    /// diagnostics (M6 re-review R4 — a prepare failure must not discard the
    /// cleanup failure).
    #[error("prepare failed: {primary} (cleanup also failed: {cleanup})")]
    PrepareFailed {
        /// The primary preparation failure (the reason the session did not
        /// become emulating).
        primary: Box<DesktopOutputError>,
        /// The cleanup failure encountered while releasing the partial
        /// session.
        cleanup: Box<DesktopOutputError>,
    },

    /// A required negotiated capability is missing for the operation.
    #[error("required output capability missing: {0}")]
    CapabilityMissing(String),

    /// The session did not reach the expected state in time.
    #[error("timed out: {0}")]
    Timeout(String),

    /// The user aborted the run (e.g. Ctrl-C during the countdown or between
    /// pattern steps).
    #[error("aborted by the user")]
    Cancelled,

    /// The current platform does not support the real backend.
    #[error("not supported on this platform: {0}")]
    UnsupportedPlatform(String),

    /// The client's predicted portal request/session handle path was not a
    /// valid D-Bus object path (M6 re-review R12). The message identifies
    /// the path construction — the handle kind, the complete constructed
    /// path, the sender component and the token — instead of surfacing later
    /// as a context-free `Invalid object path` from the match-rule builder.
    #[error(
        "invalid portal {kind} path {path:?} (sender component {sender_component:?}, \
         token {token:?}): {detail}"
    )]
    InvalidPortalPath {
        /// Which handle the path belongs to: `request` or `session`.
        kind: String,
        /// The complete predicted object path.
        path: String,
        /// The portal sender component embedded in the path.
        sender_component: String,
        /// The token embedded as the last path element.
        token: String,
        /// The underlying validation detail.
        detail: String,
    },

    /// An internal/defensive failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl DesktopOutputError {
    /// A short machine-stable category used by the CLI status lines and
    /// tests. The exact wording is not part of the stable interface; the
    /// variant is.
    #[must_use]
    pub fn category(&self) -> &'static str {
        match self {
            DesktopOutputError::NoSessionBus(_) => "no-session-bus",
            DesktopOutputError::PortalUnavailable(_) => "portal-unavailable",
            DesktopOutputError::ProtocolUnsupported { .. } => "protocol-unsupported",
            DesktopOutputError::AuthorizationCancelled => "authorization-cancelled",
            DesktopOutputError::AuthorizationRefused { .. } => "authorization-refused",
            DesktopOutputError::LibraryMissing(_) => "library-missing",
            DesktopOutputError::TransportDisconnected(_) => "transport-disconnected",
            DesktopOutputError::DevicePaused(_) => "device-paused",
            DesktopOutputError::SendFailed(_) => "send-failed",
            DesktopOutputError::ReleaseFailed(_) => "release-failed",
            // The composite preserves the primary failure's category, so
            // exit-code precedence is not flattened away (M6 re-review R4).
            DesktopOutputError::PrepareFailed { primary, .. } => primary.category(),
            DesktopOutputError::CapabilityMissing(_) => "capability-missing",
            DesktopOutputError::Timeout(_) => "timeout",
            DesktopOutputError::Cancelled => "cancelled",
            DesktopOutputError::InvalidPortalPath { .. } => "invalid-portal-path",
            DesktopOutputError::UnsupportedPlatform(_) => "unsupported-platform",
            DesktopOutputError::Internal(_) => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_stable_category() {
        let cases = [
            (
                DesktopOutputError::NoSessionBus("x".into()),
                "no-session-bus",
            ),
            (
                DesktopOutputError::PortalUnavailable("x".into()),
                "portal-unavailable",
            ),
            (
                DesktopOutputError::ProtocolUnsupported {
                    required: 4,
                    found: 2,
                    detail: "d".into(),
                },
                "protocol-unsupported",
            ),
            (
                DesktopOutputError::AuthorizationCancelled,
                "authorization-cancelled",
            ),
            (
                DesktopOutputError::AuthorizationRefused {
                    response: 2,
                    message: "m".into(),
                },
                "authorization-refused",
            ),
            (
                DesktopOutputError::LibraryMissing("x".into()),
                "library-missing",
            ),
            (
                DesktopOutputError::TransportDisconnected("x".into()),
                "transport-disconnected",
            ),
            (
                DesktopOutputError::DevicePaused("x".into()),
                "device-paused",
            ),
            (DesktopOutputError::SendFailed("x".into()), "send-failed"),
            (
                DesktopOutputError::ReleaseFailed("x".into()),
                "release-failed",
            ),
            (
                DesktopOutputError::PrepareFailed {
                    primary: Box::new(DesktopOutputError::AuthorizationCancelled),
                    cleanup: Box::new(DesktopOutputError::ReleaseFailed("c".into())),
                },
                // The composite preserves the primary's category: exit-code
                // precedence of the primary failure is not flattened away.
                "authorization-cancelled",
            ),
            (
                DesktopOutputError::CapabilityMissing("x".into()),
                "capability-missing",
            ),
            (DesktopOutputError::Timeout("x".into()), "timeout"),
            (DesktopOutputError::Cancelled, "cancelled"),
            (
                DesktopOutputError::InvalidPortalPath {
                    kind: "request".into(),
                    path: "/org/freedesktop/portal/desktop/request/1_42/m6-1-1".into(),
                    sender_component: "1_42".into(),
                    token: "m6-1-1".into(),
                    detail: "token is not a valid D-Bus object-path element".into(),
                },
                "invalid-portal-path",
            ),
            (
                DesktopOutputError::UnsupportedPlatform("x".into()),
                "unsupported-platform",
            ),
            (DesktopOutputError::Internal("x".into()), "internal"),
        ];
        for (error, expected) in cases {
            assert_eq!(error.category(), expected, "{error:?}");
        }
    }
}
