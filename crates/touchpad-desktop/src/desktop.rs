#![forbid(unsafe_code)]
//! The desktop output seam used by the CLI (M6).
//!
//! [`DesktopOutput`] is what `touchpadctl output-probe` drives: a
//! non-emitting [`probe`](DesktopOutput::probe) and an explicit
//! [`emit_pattern`](DesktopOutput::emit_pattern) that runs the fixed bounded
//! pattern and always performs the ordered cleanup (release held state →
//! disconnect → close session), preserving the primary failure and the
//! cleanup diagnostics.
//!
//! The real implementation ([`PortalDesktopOutput`], Linux-only) wires the
//! zbus RemoteDesktop portal client to the runtime-loaded libei sender
//! transport; [`UnsupportedDesktopOutput`] is the honest fallback on other
//! platforms. The emit orchestration is factored into
//! [`emit_pattern_with`], which takes injected probe/portal/transport
//! factories so the **real** orchestration (pre-flight probe, sink
//! handshake, pattern runner, ordered cleanup, outcome preservation) is
//! testable with fake portal/transport implementations — the real zbus
//! portal and libei transport are never constructed in tests
//! (M6 re-review R10).

use std::time::Duration;

use crate::emit::{run_pattern, EmitOutcome};
use crate::error::DesktopOutputError;
use crate::portal::Portal;
use crate::probe::{preflight_error, EnvProbeSource, ProbeReport, ProbeSource};
use crate::sink::PortalOutputSink;
use crate::transport::Transport;

/// The driver hooks passed into [`DesktopOutput::emit_pattern`].
pub struct EmitDriver<'a> {
    /// Sleeps for the given duration (the CLI uses it for the countdown and
    /// per-step pauses; tests use a no-op).
    pub sleeper: &'a mut dyn FnMut(Duration),
    /// Reports progress lines (the CLI prints them to stderr).
    pub progress: &'a mut dyn FnMut(&str),
    /// Whether the user asked to abort (checked between steps).
    pub cancelled: &'a dyn Fn() -> bool,
}

/// The desktop output seam.
pub trait DesktopOutput {
    /// Runs the non-emitting environment probe.
    fn probe(&self) -> ProbeReport;

    /// Runs the fixed bounded emit pattern (real desktop input!) with the
    /// ordered cleanup on every path. Only the explicit `--emit` path may
    /// call this.
    fn emit_pattern(
        &mut self,
        driver: &mut EmitDriver<'_>,
    ) -> Result<EmitOutcome, DesktopOutputError>;
}

/// The real backend: zbus RemoteDesktop portal + runtime-loaded libei
/// sender transport. Linux-only (the libei transport is a Linux surface).
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct PortalDesktopOutput {
    _marker: (),
}

#[cfg(target_os = "linux")]
impl PortalDesktopOutput {
    /// Creates the backend. Nothing connects until `probe`/`emit_pattern`
    /// is called.
    #[must_use]
    pub fn new() -> Self {
        Self { _marker: () }
    }
}

/// The real `--emit` orchestration with **injected** probe/portal/transport
/// factories.
///
/// This is the code `PortalDesktopOutput::emit_pattern` runs (with the real
/// `EnvProbeSource`, `ZbusPortal` and `NativeTransport`), factored out so
/// the exact successful [`EmitOutcome`] — `steps_emitted`, `wire_events`,
/// `skipped`, `capabilities` — survives the ordered cleanup, and so tests
/// can drive the real orchestration through fake portal/transport
/// implementations without ever constructing the real zbus portal or libei
/// transport (M6 re-review R10).
///
/// On success the pattern's own `EmitOutcome` is returned **unchanged**:
/// no field is replaced with a default. On failure the primary error
/// (pattern error, or the structured server interruption when the pattern
/// observed one) is preserved; a cleanup failure is reported as the
/// `ReleaseFailed` headline with the primary preserved in the message.
fn emit_pattern_with<P, T, PS>(
    driver: &mut EmitDriver<'_>,
    probe_source: &PS,
    portal_factory: impl FnOnce() -> Result<P, DesktopOutputError>,
    transport_factory: impl FnOnce() -> Result<T, DesktopOutputError>,
) -> Result<EmitOutcome, DesktopOutputError>
where
    P: Portal,
    T: Transport,
    PS: ProbeSource,
{
    // Pre-flight: refuse to start when the environment cannot provide
    // the session bus, the portal, or the libei library.
    let report = probe_source.probe();
    if let Some(error) = preflight_error(&report) {
        return Err(error);
    }

    let portal = portal_factory()?;
    let transport = transport_factory()?;
    let mut sink = PortalOutputSink::new(portal, transport);

    // Output preparation and authorization complete before any emission.
    // The handshake is cancellation-aware, so a signal during it aborts
    // promptly (exit 8) with the ordered cleanup; the blocking portal
    // waits are bounded and their delay before cleanup is documented.
    let capabilities = sink.prepare_cancellable(driver.cancelled)?;
    (driver.progress)(&format!(
        "negotiated capabilities: {}",
        capabilities.summary()
    ));

    // The fixed bounded pattern; the ordered cleanup runs on every path
    // (success, pattern failure, cancellation, partial send failure,
    // server interruption). A server-side interruption (device
    // pause/removal, seat removal, disconnect) observed by the pattern's
    // pumps is the structured primary failure (M6 re-review R3).
    let pattern_result = run_pattern(&mut sink, capabilities, driver);
    let server_interruption = sink.take_server_interruption();
    let release_result = sink.release_all_detailed();

    match (pattern_result, server_interruption, release_result) {
        // Success + successful cleanup: return the **exact** outcome the
        // pattern run produced — `steps_emitted`, `wire_events`, `skipped`
        // and `capabilities` all reflect the real emission (M6 re-review
        // R10), never a re-defaulted struct.
        (Ok(outcome), None, Ok(())) => Ok(outcome),
        // The structured server interruption wins over the generic wrapper
        // error `run_pattern` produced for the submit.
        (_, Some(interruption), Ok(())) => Err(interruption),
        (Err(error), None, Ok(())) => Err(error),
        // Cleanup failed: the release failure is the headline (a failed
        // release means held state may not have been released), with the
        // primary failure preserved in the message.
        (pattern, interruption, Err(release)) => {
            let primary = match (pattern, interruption) {
                (_, Some(interruption)) => Some(interruption),
                (Err(error), None) => Some(error),
                (Ok(_), None) => None,
            };
            match primary {
                Some(primary) => Err(DesktopOutputError::ReleaseFailed(format!(
                    "{primary}; cleanup also failed: {release}"
                ))),
                None => Err(DesktopOutputError::ReleaseFailed(format!(
                    "pattern completed but cleanup failed: {release}"
                ))),
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl DesktopOutput for PortalDesktopOutput {
    fn probe(&self) -> ProbeReport {
        EnvProbeSource.probe()
    }

    fn emit_pattern(
        &mut self,
        driver: &mut EmitDriver<'_>,
    ) -> Result<EmitOutcome, DesktopOutputError> {
        emit_pattern_with(
            driver,
            &EnvProbeSource,
            || {
                crate::portal_zbus::ZbusPortal::connect()
                    .map_err(|error| DesktopOutputError::PortalUnavailable(error.to_string()))
            },
            || {
                let libei = crate::ffi::Libei::load()?;
                Ok(crate::native_transport::NativeTransport::new(libei))
            },
        )
    }
}

/// The honest fallback on platforms without the real backend: probes report
/// the platform limitation and `emit_pattern` refuses.
#[derive(Debug, Clone, Default)]
pub struct UnsupportedDesktopOutput;

impl DesktopOutput for UnsupportedDesktopOutput {
    fn probe(&self) -> ProbeReport {
        let mut report = EnvProbeSource.probe();
        report.libei = crate::probe::LibeiProbeInfo {
            soname: crate::ffi::LIBEI_SONAME,
            loaded: false,
            error: Some(format!(
                "the libei output backend is not supported on {}",
                std::env::consts::OS
            )),
        };
        report
    }

    fn emit_pattern(
        &mut self,
        _driver: &mut EmitDriver<'_>,
    ) -> Result<EmitOutcome, DesktopOutputError> {
        Err(DesktopOutputError::UnsupportedPlatform(
            "the libei output backend is not built for this platform".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::capabilities::Capability;
    use crate::fake::{FakePortal, FakeProbeSource, FakeTransport};
    use crate::transport::{DeviceId, TransportEvent};

    /// Runs the real orchestration through the fake portal/transport with a
    /// no-op driver, returning the outcome (or error).
    fn run_with(
        portal: FakePortal,
        transport: FakeTransport,
    ) -> Result<EmitOutcome, DesktopOutputError> {
        let probe = FakeProbeSource {
            report: ProbeReport::available_for_tests(),
        };
        let mut sleeper = |_: Duration| {};
        let mut progress = |_: &str| {};
        let cancelled = || false;
        let mut driver = EmitDriver {
            sleeper: &mut sleeper,
            progress: &mut progress,
            cancelled: &cancelled,
        };
        emit_pattern_with(
            &mut driver,
            &probe,
            move || Ok(portal.clone()),
            move || Ok(transport.clone()),
        )
    }

    /// M6 re-review R10: the **real** `PortalDesktopOutput` orchestration —
    /// through the real `PortalOutputSink` + `run_pattern` + ordered cleanup
    /// (not `FakeDesktopOutput`, which bypasses this code entirely) — must
    /// preserve the exact successful `EmitOutcome`: with full capabilities,
    /// all 6 steps and their 11 wire events survive the cleanup, and the
    /// outcome is not re-defaulted.
    #[test]
    fn emit_pattern_preserves_nonzero_counts_through_cleanup() {
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let outcome = run_with(FakePortal::success(), transport).expect("orchestration succeeds");
        // The fixed pattern: 6 steps, 11 wire events (3 moves + click(2) +
        // scroll lifecycle(4) + click(2)), nothing skipped.
        assert_eq!(outcome.steps_emitted, 6);
        assert_eq!(outcome.wire_events, 11);
        assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
        assert!(
            outcome.capabilities.supports(Capability::RelativePointer)
                && outcome.capabilities.supports(Capability::PixelScroll)
        );
        // The real counts survived — not a defaulted struct.
        assert_ne!(outcome, EmitOutcome::default());
    }

    /// M6 re-review R10: with a pointer-only device, the skipped
    /// capabilities and their report survive the cleanup, and the emitted
    /// steps/wire-events are the real (nonzero) values — the CLI would
    /// report "3 steps emitted, 3 wire events, 3 skipped".
    #[test]
    fn emit_pattern_preserves_skipped_capabilities_through_cleanup() {
        // Pointer-only device (relative pointer capability only): the three
        // moves are emitted; the clicks and the scroll are skipped.
        let transport = FakeTransport::happy_handshake_with_caps(DeviceId(7), 1 << 0);
        let outcome = run_with(FakePortal::success(), transport).expect("orchestration succeeds");
        assert_eq!(outcome.steps_emitted, 3);
        assert_eq!(outcome.wire_events, 3);
        assert_eq!(outcome.skipped.len(), 3, "{:?}", outcome.skipped);
        // The skipped list carries the step descriptions (the exact steps
        // that could not run): the clicks and the scroll, never the moves.
        assert!(
            outcome
                .skipped
                .iter()
                .all(|description| description.contains("click") || description.contains("scroll")),
            "{:?}",
            outcome.skipped
        );
        assert!(outcome.capabilities.supports(Capability::RelativePointer));
        assert!(!outcome.capabilities.supports(Capability::PixelScroll));
        assert_ne!(outcome, EmitOutcome::default());
    }

    /// M6 re-review R10/R3: a server interruption observed by the pattern's
    /// pumps is the structured primary failure, and the cleanup still runs.
    #[test]
    fn emit_pattern_reports_a_server_interruption_as_the_primary_failure() {
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        transport.events.push_back(TransportEvent::DevicePaused {
            device: DeviceId(7),
        });
        let error = run_with(FakePortal::success(), transport).unwrap_err();
        assert!(
            matches!(&error, DesktopOutputError::DevicePaused(_)),
            "the structured interruption must win: {error:?}"
        );
    }

    /// M6 re-review R10: when the pattern succeeded but the cleanup fails,
    /// the `ReleaseFailed` headline reports that the pattern completed and
    /// the cleanup failed (the pattern's outcome is not silently lost).
    #[test]
    fn emit_pattern_reports_cleanup_failure_even_when_the_pattern_succeeded() {
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        transport.disconnect_error = Some(DesktopOutputError::TransportDisconnected(
            "injected disconnect failure".to_string(),
        ));
        let error = run_with(FakePortal::success(), transport).unwrap_err();
        assert!(
            matches!(&error, DesktopOutputError::ReleaseFailed(message)
                if message.contains("pattern completed but cleanup failed")),
            "{error:?}"
        );
    }

    /// A pre-flight probe failure refuses to start the orchestration before
    /// any portal/transport is constructed.
    #[test]
    fn emit_pattern_refuses_when_the_preflight_probe_fails() {
        let probe = FakeProbeSource {
            report: ProbeReport::available_for_tests(),
        };
        let mut report = probe.report.clone();
        report.libei.loaded = false;
        report.libei.error = Some("no libei".to_string());
        let probe = FakeProbeSource { report };
        let mut sleeper = |_: Duration| {};
        let mut progress = |_: &str| {};
        let cancelled = || false;
        let mut driver = EmitDriver {
            sleeper: &mut sleeper,
            progress: &mut progress,
            cancelled: &cancelled,
        };
        // The factories are never called.
        let error = emit_pattern_with::<FakePortal, FakeTransport, _>(
            &mut driver,
            &probe,
            || panic!("portal factory must not run"),
            || panic!("transport factory must not run"),
        )
        .unwrap_err();
        assert!(
            matches!(error, DesktopOutputError::LibraryMissing(_)),
            "{error}"
        );
    }
}
