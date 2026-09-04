//! `touchpadctl output-probe [--emit]` — the KDE Wayland output backend
//! probe (M6).
//!
//! * **Default (non-emitting dry-run):** probes the environment — session
//!   bus, RemoteDesktop portal version/device types, libei library — prints
//!   the negotiated capabilities and the exact steps `--emit` would run, and
//!   exits 0 (a completed probe is a successful probe). It never moves the
//!   pointer, clicks, or scrolls, and never touches `/dev/input`.
//! * **`--emit` (explicit opt-in):** prints a visible warning, runs a
//!   3-second countdown (cancellable with Ctrl-C), then runs the **fixed,
//!   bounded** test pattern on the real desktop through the portal + libei
//!   adapter, followed by the ordered cleanup (release held state →
//!   disconnect → close session) on every path. The backend stays
//!   `experimental/unqualified` until a reviewer measures a real run.
//!
//! Exit codes (documented in the help and README):
//!
//! | Code | Meaning |
//! | --- | --- |
//! | 0 | dry-run completed, or `--emit` pattern completed and fully cleaned up |
//! | 1 | usage |
//! | 2 | no session bus / no portal |
//! | 3 | authorization cancelled/refused |
//! | 4 | library missing / protocol too old / capability missing |
//! | 5 | transport disconnected / session timed out |
//! | 6 | a send failed (partial send failure) |
//! | 7 | releasing held state failed |
//! | 8 | aborted by the user before/during emission |
//! | 9 | unexpected/internal |

use std::time::Duration;

use touchpad_desktop::desktop::{EmitDriver, UnsupportedDesktopOutput};
use touchpad_desktop::{DesktopOutput, DesktopOutputError};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

/// How long the pre-emission countdown lasts.
pub const COUNTDOWN_SECONDS: u64 = 3;

/// Runs `output-probe [--emit]`.
pub fn run(env: &mut CommandEnv<'_>, emit: bool) -> Result<(), CommandFailure> {
    let mut output = match env.output_factory.as_mut() {
        Some(factory) => factory(),
        None => Box::new(UnsupportedDesktopOutput) as Box<dyn DesktopOutput>,
    };

    if !emit {
        // Non-emitting dry-run: a completed probe is a successful probe
        // (the findings are the report).
        let report = output.probe();
        writeln!(
            env.out,
            "{}",
            touchpad_desktop::probe::render_report(&report)
        )
        .map_err(output_error)?;
        return Ok(());
    }

    run_emit(env, &mut *output)
}

/// The explicit `--emit` path.
fn run_emit(
    env: &mut CommandEnv<'_>,
    output: &mut dyn DesktopOutput,
) -> Result<(), CommandFailure> {
    writeln!(
        env.err,
        "WARNING: --emit requested: the pointer will MOVE and CLICK, and the \
         view will SCROLL on the real desktop. This is real desktop input \
         from an experimental/unqualified backend. The pattern is fixed and \
         bounded (3 relative moves of 10/50/200 px, a primary click, a \
         smooth scroll, a secondary click)."
    )
    .map_err(output_error)?;

    let cancelled = || {
        env.stop_flag.load(std::sync::atomic::Ordering::Relaxed)
            || touchpad_linux::termination_requested()
    };

    // Countdown with per-tick cancellation.
    for remaining in (1..=COUNTDOWN_SECONDS).rev() {
        if cancelled() {
            return Err(CommandFailure::OutputCancelled(
                "aborted during the countdown; nothing was emitted".to_string(),
            ));
        }
        writeln!(
            env.err,
            "emitting in {remaining} second(s)... (Ctrl-C to cancel)"
        )
        .map_err(output_error)?;
        interruptible_sleep(Duration::from_secs(1), &cancelled);
    }
    if cancelled() {
        return Err(CommandFailure::OutputCancelled(
            "aborted during the countdown; nothing was emitted".to_string(),
        ));
    }

    let mut sleeper = |duration: Duration| interruptible_sleep(duration, &cancelled);
    let mut progress = |line: &str| {
        let _ = writeln!(env.err, "{line}");
    };
    let mut driver = EmitDriver {
        sleeper: &mut sleeper,
        progress: &mut progress,
        cancelled: &cancelled,
    };

    match output.emit_pattern(&mut driver) {
        Ok(outcome) => {
            writeln!(
                env.err,
                "emission complete: {} steps emitted, {} wire events, {} skipped (capability not negotiated)",
                outcome.steps_emitted,
                outcome.wire_events,
                outcome.skipped.len(),
            )
            .map_err(output_error)?;
            writeln!(
                env.err,
                "negotiated capabilities: {} — backend remains experimental/unqualified until measured by a reviewer",
                outcome.capabilities.summary()
            )
            .map_err(output_error)?;
            Ok(())
        }
        Err(error) => Err(output_probe_failure(error)),
    }
}

/// Sleeps in 100 ms chunks, stopping early when the user asked to cancel
/// (so Ctrl-C during the countdown or a step pause aborts promptly).
fn interruptible_sleep(duration: Duration, cancelled: &dyn Fn() -> bool) {
    let chunk = Duration::from_millis(100);
    let mut remaining = duration;
    while remaining > Duration::ZERO && !cancelled() {
        std::thread::sleep(chunk.min(remaining));
        remaining = remaining.saturating_sub(chunk);
    }
}

/// Maps a [`DesktopOutputError`] onto the documented [`CommandFailure`]
/// exit codes.
pub(crate) fn output_probe_failure(error: DesktopOutputError) -> CommandFailure {
    match error {
        DesktopOutputError::NoSessionBus(_) | DesktopOutputError::PortalUnavailable(_) => {
            CommandFailure::OutputUnavailable(error.to_string())
        }
        DesktopOutputError::AuthorizationCancelled
        | DesktopOutputError::AuthorizationRefused { .. } => {
            CommandFailure::OutputDenied(error.to_string())
        }
        DesktopOutputError::LibraryMissing(_)
        | DesktopOutputError::ProtocolUnsupported { .. }
        | DesktopOutputError::CapabilityMissing(_)
        | DesktopOutputError::UnsupportedPlatform(_) => {
            CommandFailure::OutputCapability(error.to_string())
        }
        DesktopOutputError::TransportDisconnected(_)
        | DesktopOutputError::DevicePaused(_)
        | DesktopOutputError::Timeout(_) => CommandFailure::OutputDisconnected(error.to_string()),
        // A composite prepare failure keeps the **primary** failure's
        // category/exit precedence, with the cleanup diagnostics carried
        // inside (M6 re-review R4).
        DesktopOutputError::PrepareFailed { primary, .. } => output_probe_failure(*primary),
        DesktopOutputError::SendFailed(_) => CommandFailure::OutputSendFailed(error.to_string()),
        DesktopOutputError::ReleaseFailed(_) => {
            CommandFailure::OutputReleaseFailed(error.to_string())
        }
        DesktopOutputError::Cancelled => CommandFailure::OutputCancelled(error.to_string()),
        // A locally-constructed portal path that is not a valid D-Bus object
        // path is an internal/defensive failure (the token/path generation
        // is validated by tests; the message names the path construction —
        // M6 re-review R12).
        DesktopOutputError::InvalidPortalPath { .. } => {
            CommandFailure::Unexpected(error.to_string())
        }
        DesktopOutputError::Internal(_) => CommandFailure::Unexpected(error.to_string()),
    }
}

fn output_error(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("could not write output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::env::TakeoverSeams;
    use std::sync::Arc;

    use touchpad_desktop::{EmitOutcome, FakeDesktopOutput};

    use crate::env::CommandEnv;
    use crate::exit::ExitCode;

    fn env<'a>(
        output: FakeDesktopOutput,
        out: &'a mut Vec<u8>,
        err: &'a mut Vec<u8>,
    ) -> CommandEnv<'a> {
        let mut output = Some(output);
        CommandEnv {
            sys: Rc::new(touchpad_linux::sys::mock::MockSys::new())
                as Rc<dyn touchpad_linux::sys::Sys>,
            out,
            err,
            stop_flag: Arc::new(AtomicBool::new(false)),
            recorder_factory: None,
            output_factory: Some(Box::new(move || {
                Box::new(output.take().expect("factory used once"))
            })),
            takeover: TakeoverSeams::inert(),
        }
    }

    /// M6 acceptance: the dry-run prints the probe report and exits 0
    /// without ever calling the emit path.
    #[test]
    fn dry_run_prints_the_report_and_exits_zero_without_emitting() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(FakeDesktopOutput::available(), &mut out, &mut err);
        let result = run(&mut env, false);
        assert!(result.is_ok(), "{result:?}");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("backend state: experimental/unqualified"),
            "{text}"
        );
        assert!(text.contains("--emit would:"), "{text}");
        assert!(text.contains("never touches /dev/input"), "{text}");
    }

    /// M6 acceptance: --emit runs the fixed pattern through the fake
    /// backend (which never touches the real desktop), reports the summary,
    /// and exits 0.
    #[test]
    fn emit_with_fake_backend_reports_success() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Ok(EmitOutcome {
            steps_emitted: 6,
            wire_events: 11,
            skipped: vec![],
            capabilities: touchpad_desktop::OutputCapabilities::from_device_capability_bits(
                touchpad_desktop::sink::BIND_CAPABILITY_BITS,
            ),
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let result = run(&mut env, true);
        assert!(result.is_ok(), "{result:?}");
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("WARNING: --emit requested"), "{err_text}");
        assert!(err_text.contains("emitting in 3 second(s)"), "{err_text}");
        assert!(
            err_text.contains("emission complete: 6 steps emitted, 11 wire events"),
            "{err_text}"
        );
        assert!(err_text.contains("experimental/unqualified"), "{err_text}");
    }

    /// M6: a send failure maps to exit 6 (partial send failure).
    #[test]
    fn emit_send_failure_maps_to_exit_6() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Err(DesktopOutputError::SendFailed(
            "injected send failure".to_string(),
        ));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        assert!(failure.to_string().contains("send failed"), "{failure}");
    }

    /// M6: a release failure maps to exit 7.
    #[test]
    fn emit_release_failure_maps_to_exit_7() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Err(DesktopOutputError::ReleaseFailed(
            "injected release failure".to_string(),
        ));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
        assert!(failure.to_string().contains("release failed"), "{failure}");
    }

    /// M6: cancelled authorization maps to exit 3.
    #[test]
    fn emit_authorization_cancelled_maps_to_exit_3() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Err(DesktopOutputError::AuthorizationCancelled);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Permission, "{failure}");
    }

    /// M6: a missing library maps to exit 4.
    #[test]
    fn emit_library_missing_maps_to_exit_4() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Err(DesktopOutputError::LibraryMissing(
            "libei.so.1 not found".to_string(),
        ));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::NoCandidate, "{failure}");
    }

    /// M6: a transport disconnect maps to exit 5.
    #[test]
    fn emit_transport_disconnect_maps_to_exit_5() {
        let mut output = FakeDesktopOutput::available();
        output.emit_result = Err(DesktopOutputError::TransportDisconnected(
            "server closed the connection".to_string(),
        ));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(output, &mut out, &mut err);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
    }

    /// M6: user cancellation during the countdown maps to exit 8 and emits
    /// nothing.
    #[test]
    fn cancellation_during_countdown_maps_to_exit_8() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(FakeDesktopOutput::available(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        let failure = run(&mut env, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
        assert!(
            failure.to_string().contains("nothing was emitted"),
            "{failure}"
        );
    }

    /// M6 re-review R2: the dry-run never installs the handler (it holds
    /// nothing that needs cleanup) — verified via the classification used by
    /// the binary entry point.
    #[test]
    fn termination_handler_is_installed_only_for_commands_with_live_resources() {
        use crate::Command;
        assert!(crate::command_needs_termination_handler(&Command::Record {
            device: "/dev/input/event0".into(),
            output: "t.jsonl".into(),
            grab: false,
        }));
        assert!(crate::command_needs_termination_handler(
            &Command::OutputProbe { emit: true }
        ));
        assert!(crate::command_needs_termination_handler(
            &Command::ServiceRun {
                settings: "settings.json".into(),
            }
        ));
        assert!(!crate::command_needs_termination_handler(
            &Command::OutputProbe { emit: false }
        ));
        assert!(!crate::command_needs_termination_handler(&Command::Devices));
        assert!(!crate::command_needs_termination_handler(
            &Command::Replay {
                input: "t.jsonl".into(),
            }
        ));
        assert!(!crate::command_needs_termination_handler(&Command::Help));
    }

    /// M6 re-review R4: a composite prepare failure maps to the **primary**
    /// failure's exit code (the cleanup diagnostics are carried inside, not
    /// flattened into an unrelated code).
    #[test]
    fn prepare_failure_maps_to_the_primary_exit_code() {
        let composite = DesktopOutputError::PrepareFailed {
            primary: Box::new(DesktopOutputError::AuthorizationCancelled),
            cleanup: Box::new(DesktopOutputError::ReleaseFailed("close failed".into())),
        };
        let failure = output_probe_failure(composite);
        assert_eq!(failure.exit_code(), ExitCode::Permission, "{failure}");
        let composite = DesktopOutputError::PrepareFailed {
            primary: Box::new(DesktopOutputError::TransportDisconnected("gone".into())),
            cleanup: Box::new(DesktopOutputError::ReleaseFailed("close failed".into())),
        };
        let failure = output_probe_failure(composite);
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
    }

    /// M6 re-review R3: a device pause after handshake maps to the
    /// transport exit code (5).
    #[test]
    fn device_pause_maps_to_exit_5() {
        let failure = output_probe_failure(DesktopOutputError::DevicePaused(
            "the EIS device 7 was paused by the server".into(),
        ));
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
    }
}
