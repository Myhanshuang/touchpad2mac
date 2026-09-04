//! Production-facing readiness, diagnostics and hardware qualification tools.
//!
//! These commands are deliberately read-only with respect to input devices:
//! they probe capabilities but never grab a touchpad, emit desktop input, or
//! record keyboard key codes.

#![forbid(unsafe_code)]

use std::path::Path;

use serde_json::{json, Value};
use touchpad_desktop::probe::{preflight_error, EnvProbeSource};
use touchpad_desktop::ProbeSource;
use touchpad_linux::{enumerate, ProbeVerdict};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

fn enumerate_devices(
    env: &CommandEnv<'_>,
) -> Result<Vec<touchpad_linux::ProbeReport>, CommandFailure> {
    enumerate(&*env.sys).map_err(|error| {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("permission") {
            CommandFailure::Permission(message)
        } else {
            CommandFailure::InputDir(message)
        }
    })
}

/// Runs a compact, read-only production readiness check.
pub(crate) fn run_doctor(env: &mut CommandEnv<'_>, settings: &Path) -> Result<(), CommandFailure> {
    let settings_result = crate::cmd::settings::read_settings(settings);
    match &settings_result {
        Ok(_) => writeln!(env.out, "[ok] settings: {}", settings.display()),
        Err(error) => writeln!(env.out, "[fail] settings: {error}"),
    }
    .map_err(write_error)?;

    let reports = enumerate_devices(env)?;
    let candidates: Vec<_> = reports
        .iter()
        .filter_map(|report| {
            report
                .candidate_descriptor()
                .map(|descriptor| (report, descriptor))
        })
        .collect();
    if candidates.is_empty() {
        writeln!(env.out, "[fail] touchpad: no usable Type-B candidate").map_err(write_error)?;
    } else if candidates.len() == 1 {
        let (report, descriptor) = candidates[0];
        writeln!(
            env.out,
            "[ok] touchpad: {} ({}, {:04x}:{:04x}, profile {})",
            descriptor.name,
            report.path.display(),
            report.id.vendor,
            report.id.product,
            descriptor.profile.name
        )
        .map_err(write_error)?;
    } else {
        writeln!(
            env.out,
            "[warn] touchpad: {} candidates; service will refuse to guess",
            candidates.len()
        )
        .map_err(write_error)?;
    }

    let output = EnvProbeSource.probe();
    let output_error = preflight_error(&output);
    match &output_error {
        None => writeln!(
            env.out,
            "[ok] desktop output: session bus + RemoteDesktop portal + {}",
            output.libei.soname
        ),
        Some(error) => writeln!(env.out, "[fail] desktop output: {error}"),
    }
    .map_err(write_error)?;

    writeln!(
        env.out,
        "[info] DWT: keyboard discovery is performed read-only at runtime; key codes are never written to diagnostics or touch traces"
    )
    .map_err(write_error)?;

    settings_result?;
    if candidates.is_empty() {
        return Err(CommandFailure::NoCandidate(
            "doctor found no usable touchpad candidate".to_string(),
        ));
    }
    if candidates.len() > 1 {
        return Err(CommandFailure::NoCandidate(
            "doctor found multiple touchpad candidates; select one explicitly for development or add a hardware quirk that disambiguates the machine"
                .to_string(),
        ));
    }
    if let Some(error) = output_error {
        return Err(CommandFailure::OutputCapability(error.to_string()));
    }
    writeln!(env.out, "READY: production prerequisites passed").map_err(write_error)
}

/// Writes a privacy-preserving JSON diagnostics bundle.
pub(crate) fn run_diagnostics(
    env: &mut CommandEnv<'_>,
    output: &Path,
) -> Result<(), CommandFailure> {
    let reports = enumerate_devices(env)?;
    let desktop = EnvProbeSource.probe();
    let devices = reports.iter().map(device_json).collect::<Vec<_>>();
    let document = json!({
        "schema_version": 1,
        "touchpad2mac_version": env!("CARGO_PKG_VERSION"),
        "privacy": {
            "contains_keyboard_key_codes": false,
            "contains_touch_contents": false,
            "note": "This static bundle contains device/session metadata only. Attach an explicit touch trace separately when reproduction requires it."
        },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "wayland_display": std::env::var("WAYLAND_DISPLAY").ok(),
            "session_type": std::env::var("XDG_SESSION_TYPE").ok(),
            "desktop": std::env::var("XDG_CURRENT_DESKTOP").ok(),
        },
        "desktop_output": {
            "session_bus": desktop.session_bus.as_ref().map(|_| "reachable").unwrap_or("unavailable"),
            "portal": desktop.portal.as_ref().map(|value| json!({
                "interface_version": value.interface_version,
                "available_device_types": value.available_device_types,
                "pointer_available": value.pointer_available(),
            })).unwrap_or_else(|error| json!({"error": error})),
            "libei": {
                "soname": desktop.libei.soname,
                "loaded": desktop.libei.loaded,
                "error": desktop.libei.error,
            }
        },
        "input_devices": devices,
    });
    write_json(output, &document)?;
    writeln!(env.out, "wrote diagnostics bundle: {}", output.display()).map_err(write_error)
}

/// Writes a machine-specific qualification checklist suitable for attaching
/// to a hardware-support pull request.
pub(crate) fn run_qualify(env: &mut CommandEnv<'_>, output: &Path) -> Result<(), CommandFailure> {
    let reports = enumerate_devices(env)?;
    let candidates = reports
        .iter()
        .filter(|report| report.candidate_descriptor().is_some())
        .map(device_json)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(CommandFailure::NoCandidate(
            "cannot create a qualification report without a usable touchpad candidate".to_string(),
        ));
    }
    let cases = [
        "pointer-motion",
        "single-tap",
        "double-tap",
        "tap-drag",
        "two-finger-scroll",
        "three-finger-middle-click",
        "three-finger-drag",
        "three-finger-tap-drag-boundary",
        "disable-while-typing",
        "palm-rejection",
        "suspend-resume",
        "device-hot-unplug-replug",
        "controlled-sigterm-cleanup",
    ];
    let checks = cases
        .into_iter()
        .map(|name| json!({"name": name, "status": "untested", "notes": ""}))
        .collect::<Vec<_>>();
    let document = json!({
        "schema_version": 1,
        "touchpad2mac_version": env!("CARGO_PKG_VERSION"),
        "hardware": candidates,
        "tests": checks,
        "allowed_status": ["untested", "pass", "fail", "not-applicable"],
        "submission_note": "Run the listed cases on real hardware, update only status/notes, and attach this file with diagnostics JSON to a hardware qualification PR."
    });
    write_json(output, &document)?;
    writeln!(
        env.out,
        "wrote qualification checklist: {}",
        output.display()
    )
    .map_err(write_error)
}

fn device_json(report: &touchpad_linux::ProbeReport) -> Value {
    let verdict = match &report.verdict {
        ProbeVerdict::Candidate { descriptor } => json!({
            "kind": "candidate",
            "profile": descriptor.profile.name,
            "quirks": descriptor.profile.quirks,
        }),
        ProbeVerdict::Rejected { reasons } => json!({"kind": "rejected", "reasons": reasons}),
        ProbeVerdict::Inaccessible { error } => json!({"kind": "inaccessible", "error": error}),
    };
    json!({
        "path": report.path,
        "name": report.name,
        "vendor_id": report.id.vendor,
        "product_id": report.id.product,
        "version": report.id.version,
        "slot_count": report.slot_count,
        "verdict": verdict,
        "evidence": report.evidence,
    })
}

fn write_json(path: &Path, value: &Value) -> Result<(), CommandFailure> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        CommandFailure::Unexpected(format!("could not serialize JSON: {error}"))
    })?;
    std::fs::write(path, bytes).map_err(|error| {
        CommandFailure::Config(format!("could not write {}: {error}", path.display()))
    })
}

fn write_error(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("could not write output: {error}"))
}
