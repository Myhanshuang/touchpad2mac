#![forbid(unsafe_code)]
//! The XDG RemoteDesktop portal seam (M6).
//!
//! The portal is the authorization and transport hand-off layer: it creates
//! the remote-desktop session, asks the user for authorization (`Start`),
//! and returns a socket fd to the EIS implementation (`ConnectToEIS`, added
//! in **interface version 2** — the version observed on this host) that the
//! libei sender transport then connects to.
//!
//! The real implementation is the zbus-based client
//! ([`crate::portal_zbus::ZbusPortal`]); the fake is
//! [`crate::fake::FakePortal`]. Every portal step can fail with an honest
//! structured result (refusal, cancellation, missing bus/portal/protocol).

use crate::error::DesktopOutputError;

/// Device-type bitmask values from the RemoteDesktop portal specification
/// (`AvailableDeviceTypes`).
pub mod device_types {
    /// Keyboard device type (bit 0).
    pub const KEYBOARD: u32 = 1;
    /// Pointer device type (bit 1) — the only type M6 requests.
    pub const POINTER: u32 = 2;
    /// Touchscreen device type (bit 2).
    pub const TOUCHSCREEN: u32 = 4;
}

/// The EIS socket file descriptor returned by `ConnectToEIS`. Ownership is
/// transferred to the transport ([`crate::transport::Transport::connect`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EisFd(pub i32);

/// A remote-desktop session handle (the portal object path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalSession(pub String);

/// The RemoteDesktop portal seam.
///
/// * `create_session` — `CreateSession(options)`, no user prompt yet.
/// * `select_devices` — `SelectDevices(session, options)` requesting the
///   given device-type bitmask (the pointer type).
/// * `start` — `Start(session, parent_window, options)`; **this is where the
///   authorization dialog appears**. A user cancel is reported as
///   [`DesktopOutputError::AuthorizationCancelled`], a refusal as
///   [`DesktopOutputError::AuthorizationRefused`].
/// * `connect_to_eis` — `ConnectToEIS(session, options)` returning the EIS
///   fd; must be called after a successful `start`.
/// * `close_session` — `org.freedesktop.portal.Session.Close`; idempotent at
///   the adapter level (no-op when the session was never created).
pub trait Portal {
    /// Create a remote desktop session.
    fn create_session(&mut self) -> Result<PortalSession, DesktopOutputError>;

    /// Request the given device types (bitmask) for the session.
    fn select_devices(
        &mut self,
        session: &PortalSession,
        types: u32,
    ) -> Result<(), DesktopOutputError>;

    /// Start the session (authorization).
    fn start(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError>;

    /// Return the EIS socket fd for the session.
    fn connect_to_eis(&mut self, session: &PortalSession) -> Result<EisFd, DesktopOutputError>;

    /// Close the session.
    fn close_session(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_type_constants_match_the_portal_spec() {
        assert_eq!(device_types::KEYBOARD, 1);
        assert_eq!(device_types::POINTER, 2);
        assert_eq!(device_types::TOUCHSCREEN, 4);
    }
}
