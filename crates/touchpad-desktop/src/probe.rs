#![forbid(unsafe_code)]
//! Environment probing for the KDE Wayland output backend (M6 required
//! outcome 6).
//!
//! `touchpadctl output-probe` (default, non-emitting) prints this report:
//! what the environment provides (session bus, RemoteDesktop portal version,
//! device types, libei library), what would be negotiated, the exact steps
//! `--emit` would run, and the backend's honest status —
//! **`experimental/unqualified`** until a reviewer actually runs and
//! measures `--emit` (PHASE2_PLAN.md §5 M6).
//!
//! Probing is read-only: it connects to the session bus and reads
//! properties, and dlopens the libei library; it never creates a portal
//! session, never requests authorization, and never emits input. It also
//! never touches `/dev/input`.

use crate::capabilities::{libei_capability_bits, OutputCapabilities};
use crate::error::DesktopOutputError;

/// Read-only observations about the platform/session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformObservation {
    /// `cfg!(target_os)` of the build.
    pub target_os: &'static str,
    /// `$WAYLAND_DISPLAY` (when set).
    pub wayland_display: Option<String>,
    /// `$XDG_SESSION_TYPE` (when set).
    pub session_type: Option<String>,
    /// `$XDG_CURRENT_DESKTOP` (when set).
    pub desktop: Option<String>,
}

impl PlatformObservation {
    /// Collects the observations from the process environment.
    #[must_use]
    pub fn collect() -> Self {
        Self {
            target_os: std::env::consts::OS,
            wayland_display: std::env::var("WAYLAND_DISPLAY").ok(),
            session_type: std::env::var("XDG_SESSION_TYPE").ok(),
            desktop: std::env::var("XDG_CURRENT_DESKTOP").ok(),
        }
    }
}

/// Portal capability observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalProbeInfo {
    /// The `org.freedesktop.portal.RemoteDesktop` interface version.
    pub interface_version: u32,
    /// The `AvailableDeviceTypes` bitmask.
    pub available_device_types: u32,
}

impl PortalProbeInfo {
    /// Whether the portal advertises the pointer device type.
    #[must_use]
    pub fn pointer_available(&self) -> bool {
        self.available_device_types & crate::portal::device_types::POINTER != 0
    }
}

/// libei library observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibeiProbeInfo {
    /// The soname the adapter loads.
    pub soname: &'static str,
    /// Whether the library loaded successfully.
    pub loaded: bool,
    /// The load failure reason when `loaded` is false.
    pub error: Option<String>,
}

/// The full non-emitting probe report.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    /// Platform/session observations.
    pub platform: PlatformObservation,
    /// Session bus reachability: `Ok(())` or the reason it is missing.
    pub session_bus: Result<(), String>,
    /// Portal availability: `Ok(PortalProbeInfo)` or the reason it is
    /// missing.
    pub portal: Result<PortalProbeInfo, String>,
    /// libei availability.
    pub libei: LibeiProbeInfo,
    /// The capabilities the adapter would request/negotiate (confirmed only
    /// by an actual `--emit` run).
    pub requested_capabilities: OutputCapabilities,
    /// The exact steps `--emit` would perform.
    pub steps: Vec<String>,
    /// The honest backend status: `experimental/unqualified` until the
    /// reviewer measures a real `--emit`.
    pub backend_state: &'static str,
}

impl ProbeReport {
    /// The steps `--emit` would run, in order.
    pub const EMIT_STEPS: [&'static str; 8] = [
        "connect to the D-Bus session bus",
        "create a RemoteDesktop session (CreateSession)",
        "SelectDevices(pointer) — request the pointer device type",
        "Start — the portal shows an authorization dialog",
        "ConnectToEIS — obtain the EIS socket fd (portal interface v2)",
        "libei sender handshake: bind pointer/button/scroll, wait for a resumed device",
        "emit the fixed bounded pattern (3 pointer deltas, primary click, smooth scroll, secondary click)",
        "release all held state, disconnect, close the session",
    ];

    /// The honest status printed until a reviewer measures `--emit`.
    pub const UNQUALIFIED: &'static str = "experimental/unqualified";

    /// A fully-available canned report used by the test fakes (never by the
    /// real binary).
    #[must_use]
    pub fn available_for_tests() -> Self {
        Self {
            platform: PlatformObservation {
                target_os: std::env::consts::OS,
                wayland_display: Some("wayland-0".to_string()),
                session_type: Some("wayland".to_string()),
                desktop: Some("KDE".to_string()),
            },
            session_bus: Ok(()),
            portal: Ok(PortalProbeInfo {
                interface_version: 2,
                available_device_types: 7,
            }),
            libei: LibeiProbeInfo {
                soname: "libei.so.1",
                loaded: true,
                error: None,
            },
            requested_capabilities: OutputCapabilities::from_device_capability_bits(
                libei_capability_bits::POINTER
                    | libei_capability_bits::BUTTON
                    | libei_capability_bits::SCROLL,
            ),
            steps: Self::EMIT_STEPS
                .iter()
                .map(|step| (*step).to_string())
                .collect(),
            backend_state: Self::UNQUALIFIED,
        }
    }
}

/// The probe seam (the real implementation probes the environment; the fake
/// returns a canned report).
pub trait ProbeSource {
    /// Runs the probe and returns the report.
    fn probe(&self) -> ProbeReport;
}

/// The real probe: session bus + portal properties via zbus, libei via
/// dlopen. Read-only; never creates a session and never emits.
#[derive(Debug, Clone, Default)]
pub struct EnvProbeSource;

impl ProbeSource for EnvProbeSource {
    fn probe(&self) -> ProbeReport {
        let session_bus = match crate::portal_zbus::session_bus_reachable() {
            Ok(()) => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        let portal = match &session_bus {
            Ok(()) => match crate::portal_zbus::probe_portal() {
                Ok(info) => Ok(info),
                Err(error) => Err(error.to_string()),
            },
            Err(reason) => Err(format!("no session bus: {reason}")),
        };
        let libei = crate::ffi::Libei::load();
        let libei_info = match libei {
            Ok(_) => LibeiProbeInfo {
                soname: crate::ffi::LIBEI_SONAME,
                loaded: true,
                error: None,
            },
            Err(error) => LibeiProbeInfo {
                soname: crate::ffi::LIBEI_SONAME,
                loaded: false,
                error: Some(error.to_string()),
            },
        };
        ProbeReport {
            platform: PlatformObservation::collect(),
            session_bus,
            portal,
            libei: libei_info,
            requested_capabilities: OutputCapabilities::from_device_capability_bits(
                libei_capability_bits::POINTER
                    | libei_capability_bits::BUTTON
                    | libei_capability_bits::SCROLL,
            ),
            steps: ProbeReport::EMIT_STEPS
                .iter()
                .map(|step| (*step).to_string())
                .collect(),
            backend_state: ProbeReport::UNQUALIFIED,
        }
    }
}

/// Renders a probe report as stable, human-readable text lines (used by
/// `output-probe`'s dry-run).
pub fn render_report(report: &ProbeReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("backend state: {}", report.backend_state));
    lines.push(format!(
        "platform: {} (WAYLAND_DISPLAY={}, XDG_SESSION_TYPE={}, XDG_CURRENT_DESKTOP={})",
        report.platform.target_os,
        option_or(report.platform.wayland_display.as_deref(), "unset"),
        option_or(report.platform.session_type.as_deref(), "unset"),
        option_or(report.platform.desktop.as_deref(), "unset"),
    ));
    match &report.session_bus {
        Ok(()) => lines.push("session bus: reachable".to_string()),
        Err(reason) => lines.push(format!("session bus: MISSING ({reason})")),
    }
    match &report.portal {
        Ok(info) => {
            lines.push(format!(
                "RemoteDesktop portal: available (interface version {}, device types {}{})",
                info.interface_version,
                info.available_device_types,
                if info.pointer_available() {
                    " [pointer available]"
                } else {
                    " [pointer NOT available]"
                },
            ));
        }
        Err(reason) => lines.push(format!("RemoteDesktop portal: UNAVAILABLE ({reason})")),
    }
    if report.libei.loaded {
        lines.push(format!("libei: {} loadable", report.libei.soname));
    } else {
        lines.push(format!(
            "libei: {} NOT loadable ({})",
            report.libei.soname,
            report.libei.error.as_deref().unwrap_or("unknown")
        ));
    }
    lines.push(format!(
        "requested capabilities (negotiated only by an actual --emit): {}",
        report.requested_capabilities.summary()
    ));
    lines.push("--emit would:".to_string());
    for (index, step) in report.steps.iter().enumerate() {
        lines.push(format!("  {}. {step}", index + 1));
    }
    lines.push(
        "note: the backend stays experimental/unqualified until a reviewer runs and measures --emit".to_string(),
    );
    lines.push(
        "note: output-probe never touches /dev/input and never emits input in dry-run mode"
            .to_string(),
    );
    lines.join("\n")
}

fn option_or<'a>(value: Option<&'a str>, fallback: &'a str) -> &'a str {
    value.unwrap_or(fallback)
}

/// Maps a probe report onto an actionable [`DesktopOutputError`] for the
/// `--emit` path's pre-flight check: the emit path refuses to start when
/// the session bus, the portal, or libei are missing.
#[must_use]
pub fn preflight_error(report: &ProbeReport) -> Option<DesktopOutputError> {
    if let Err(reason) = &report.session_bus {
        return Some(DesktopOutputError::NoSessionBus(reason.clone()));
    }
    if let Err(reason) = &report.portal {
        return Some(DesktopOutputError::PortalUnavailable(reason.clone()));
    }
    if !report.libei.loaded {
        return Some(DesktopOutputError::LibraryMissing(
            report
                .libei
                .error
                .clone()
                .unwrap_or_else(|| report.libei.soname.to_string()),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_report_is_complete_and_unqualified() {
        let report = ProbeReport::available_for_tests();
        assert!(report.session_bus.is_ok());
        assert_eq!(report.portal.as_ref().unwrap().interface_version, 2);
        assert!(report.portal.as_ref().unwrap().pointer_available());
        assert!(report.libei.loaded);
        assert_eq!(report.backend_state, ProbeReport::UNQUALIFIED);
        assert_eq!(report.steps.len(), ProbeReport::EMIT_STEPS.len());
        assert_eq!(preflight_error(&report), None);
    }

    #[test]
    fn missing_library_is_a_preflight_blocker() {
        let mut report = ProbeReport::available_for_tests();
        report.libei.loaded = false;
        report.libei.error = Some("no such library".to_string());
        assert!(matches!(
            preflight_error(&report),
            Some(DesktopOutputError::LibraryMissing(_))
        ));
    }

    #[test]
    fn missing_bus_and_portal_are_preflight_blockers() {
        let mut report = ProbeReport::available_for_tests();
        report.session_bus = Err("no bus".to_string());
        assert!(matches!(
            preflight_error(&report),
            Some(DesktopOutputError::NoSessionBus(_))
        ));
        let mut report = ProbeReport::available_for_tests();
        report.portal = Err("no portal".to_string());
        assert!(matches!(
            preflight_error(&report),
            Some(DesktopOutputError::PortalUnavailable(_))
        ));
    }

    #[test]
    fn render_report_covers_every_finding() {
        let text = render_report(&ProbeReport::available_for_tests());
        for needle in [
            "backend state: experimental/unqualified",
            "session bus: reachable",
            "RemoteDesktop portal: available",
            "libei: libei.so.1 loadable",
            "--emit would:",
            "never touches /dev/input",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    #[test]
    fn env_probe_is_side_effect_free_and_structured() {
        // The real probe must produce a structured report on any machine
        // (the findings may vary, but it never panics and never emits).
        let report = EnvProbeSource.probe();
        assert_eq!(report.backend_state, ProbeReport::UNQUALIFIED);
        assert_eq!(report.steps.len(), ProbeReport::EMIT_STEPS.len());
        let _ = report.session_bus;
        let _ = report.portal;
    }
}
