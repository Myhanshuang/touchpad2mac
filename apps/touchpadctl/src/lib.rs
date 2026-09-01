//! # touchpadctl — Touchpad Runtime Phase 1 command-line tool (M5)
//!
//! The CLI vertical slice:
//!
//! ```text
//! touchpadctl devices              — enumerate /dev/input/event* and explain
//!                                     each verdict (candidate / rejected /
//!                                     inaccessible)
//! touchpadctl inspect DEVICE       — probe one device node and show identity,
//!                                     capabilities, axes, slot count, verdict
//! touchpadctl record DEVICE OUTPUT — record raw evdev events into a versioned
//!                                     JSON Lines trace ([--grab] opt-in)
//! touchpadctl replay INPUT         — offline replay of a raw trace through the
//!                                     exact same Type-B decoder used live
//! ```
//!
//! Design invariants (M5):
//!
//! * **Recorder before decoder.** [`cmd::record`] attaches the raw-event
//!   recorder to the runtime *in front of* the decoder: every raw event read
//!   from the device is written to the trace before the decoder sees it, so a
//!   decoder bug can never lose the raw input needed to reproduce it.
//! * **Same decoder for replay.** [`cmd::replay`] drives
//!   [`touchpad_linux::TypeBDecoder`] through the
//!   [`touchpad_trace::ReplayDriver`] — the exact state machine used by live
//!   input; there is no second decoder.
//! * **Controlled signal stop.** `record` installs a `SIGINT`/`SIGTERM`
//!   handler ([`touchpad_linux::install_termination_handler`]) that records
//!   a stop request in a **process-lifetime static** — the async handler
//!   dereferences no caller-owned memory (M5 re-review R1); the blocked read
//!   is interrupted (`EINTR`) and mapped to a graceful stop, and the stop
//!   state is also polled between steps. Every exit path (normal, signal,
//!   EOF/unplug, decoder/recorder error) runs **one** ordered finalization
//!   performed by the runtime: stop work → semantic-output lifecycle no-op →
//!   recorder finish (which flushes) plus best-effort recorder destruction,
//!   before the device release → ungrab at most once → close regardless of
//!   prior errors → structured status. The returned failure is truthful
//!   about cleanup (M5 review R3): exit 8 is only produced when the trace
//!   finalization and the device release both succeeded; otherwise the
//!   recorder (7) or device-release (6) failure preserves every diagnostic.
//!   `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
//!   cleanup.
//! * **Offline, hardware-free replay.** `replay` never touches `/dev/input`;
//!   it reads the trace file only and runs on CI without a desktop session.
//!   All tests use the mockable [`touchpad_linux::sys`] seam (the only real
//!   OS surfaces exercised are the side-effect-free Linux FFI tests —
//!   `sigaction`, `raise`, `read_dir`/`open` on nonexistent paths) and no
//!   test opens or grabs a real device.
//!
//! Every command is implemented against a [`CommandEnv`] carrying the
//! [`touchpad_linux::sys::Sys`] seam, the output writers, and the stop flag,
//! so the whole CLI is testable in-process (library-level command runner)
//! with mocks and fixtures.
//!
//! This crate is `unsafe`-free.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod args;
pub mod cmd;
pub mod desktop_backend;
pub mod env;
pub mod exit;
pub mod output;

pub use args::{parse_args, Command};
pub use env::CommandEnv;
pub use exit::{CommandFailure, ExitCode};
pub use output::{CountingSink, FramePrinterSink};

/// Dispatches a parsed command onto the command runner with `env`, mapping
/// every outcome to a [`CommandFailure`] whose [`CommandFailure::exit_code`]
/// is the process exit code (0 on success).
pub fn run_command(env: &mut CommandEnv<'_>, command: &Command) -> Result<(), CommandFailure> {
    match command {
        Command::Help => {
            args::print_help(&mut *env.out).map_err(|error| {
                CommandFailure::Unexpected(format!("could not write output: {error}"))
            })?;
            Ok(())
        }
        Command::Devices => cmd::devices::run(env),
        Command::Inspect { device } => cmd::inspect::run(env, device),
        Command::Record {
            device,
            output,
            grab,
        } => cmd::record::run(env, device, output, *grab),
        Command::Replay { input } => cmd::replay::run(env, input),
        Command::OutputProbe { emit } => cmd::output_probe::run(env, *emit),
        Command::WindowsProbe => {
            writeln!(
                env.out,
                "{}",
                touchpad_windows::render_windows_support(&touchpad_windows::probe_windows_support())
            )
            .map_err(|error| {
                CommandFailure::Unexpected(format!("could not write output: {error}"))
            })?;
            Ok(())
        }
        Command::ConfigCheck { input } => cmd::config::run_check(env, input),
        Command::ServicePreflight { input } => cmd::config::run_preflight(env, input),
        Command::FeelDefault { output } => cmd::feel::run_default(env, output),
        Command::FeelCheck { input } => cmd::feel::run_check(env, input),
        Command::FeelShow { input } => cmd::feel::run_show(env, input),
        Command::FeelSet {
            input,
            output,
            edits,
        } => cmd::feel::run_set(env, input, output, edits),
        Command::FeelGui { input, output } => cmd::feel::run_gui(env, input, output),
        Command::SettingsDefault { output } => cmd::settings::run_default(env, output),
        Command::SettingsMacos { output } => cmd::settings::run_macos(env, output),
        Command::SettingsCheck { input } => cmd::settings::run_check(env, input),
        Command::SettingsShow { input } => cmd::settings::run_show(env, input),
        Command::SettingsSet {
            input,
            output,
            edits,
        } => cmd::settings::run_set(env, input, output, edits),
        Command::SettingsPatch { input, edits } => cmd::settings::run_patch(env, input, edits),
        Command::SettingsGui { input, output } => cmd::settings::run_gui(env, input, output),
        Command::Takeover {
            device,
            trace,
            max_duration_seconds,
            profile,
            feel_config,
            settings,
            watch_settings,
        } => cmd::takeover::run(
            env,
            device.as_deref(),
            trace,
            *max_duration_seconds,
            profile,
            cmd::takeover::ProfileInputs {
                feel_config_path: feel_config.as_deref(),
                settings_path: settings.as_deref(),
                watch_settings: *watch_settings,
            },
        ),
    }
}

/// Whether the command needs the controlled `SIGINT`/`SIGTERM` handler:
/// exactly the commands with a blocking wait and ordered cleanup that must
/// run on a signal — `record` (device read + recorder/device release),
/// `output-probe --emit` (real desktop emission whose cleanup releases held
/// state and closes the session, M6 re-review R2), and `takeover` (the M10
/// bounded live takeover: exclusive grab, real desktop emission, recorder,
/// and the unified ordered shutdown must run on a real Ctrl-C/SIGTERM,
/// M10_TASK.md §2). The non-emitting `output-probe` dry-run keeps the
/// default dispositions (it holds nothing that needs cleanup).
#[must_use]
pub fn command_needs_termination_handler(command: &Command) -> bool {
    matches!(
        command,
        Command::Record { .. } | Command::OutputProbe { emit: true } | Command::Takeover { .. }
    )
}
