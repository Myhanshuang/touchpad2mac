//! Stable process exit codes and the structured command failure type.
//!
//! The codes are documented in the CLI help text and the README; they are
//! part of the CLI's stable interface (scripts may branch on them).

/// Stable process exit codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    /// Success.
    Success = 0,
    /// Usage / argument error.
    Usage = 1,
    /// Input directory or device node not found (no `/dev/input`).
    InputDir = 2,
    /// Permission denied reading the input directory or device node.
    Permission = 3,
    /// No touchpad candidate (or the inspected device is not a candidate).
    NoCandidate = 4,
    /// Trace file error (missing, corrupt, schema mismatch, time regression).
    Trace = 5,
    /// Device stream error (EOF/unplug, torn read, decoder failure) or a
    /// device-release failure (ungrab/close failed during record cleanup, M5
    /// review R3).
    Stream = 6,
    /// Recorder error (trace output could not be written or finalized, M5
    /// review R3).
    Recorder = 7,
    /// Stopped by SIGINT/SIGTERM (controlled stop). Only produced when the
    /// ordered finalization — recorder finish/flush and the device release —
    /// actually succeeded (M5 review R3); otherwise
    /// [`CommandFailure::RecorderFinalize`] (7) or
    /// [`CommandFailure::DeviceRelease`] (6) is returned with the full
    /// diagnostic.
    Stopped = 8,
    /// Unexpected/internal error.
    Unexpected = 9,
}

impl ExitCode {
    /// The numeric process exit code.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// A structured failure of a command, mapped to a stable [`ExitCode`].
#[derive(Debug, thiserror::Error)]
pub enum CommandFailure {
    /// Bad command line (unknown command/flag, wrong arity).
    #[error(transparent)]
    Usage(#[from] crate::args::UsageError),
    /// Input directory or device node missing.
    #[error("{0}")]
    InputDir(String),
    /// Permission denied.
    #[error("{0}")]
    Permission(String),
    /// No touchpad candidate / the device is not a candidate.
    #[error("{0}")]
    NoCandidate(String),
    /// Trace file error.
    #[error("trace error: {0}")]
    Trace(#[from] touchpad_trace::TraceError),
    /// The trace could not be decoded (exit code [`ExitCode::Trace`]): an
    /// unsupported schema/clock, an unrepresentable timestamp, an invalid
    /// device header, or an unresolved synchronization loss at the end of the
    /// trace.
    #[error("{0}")]
    Replay(String),
    /// Device stream / decoder error.
    #[error("device stream error: {0}")]
    Stream(String),
    /// Recorder error.
    #[error("recorder error: {0}")]
    Recorder(touchpad_linux::RecorderError),
    /// Controlled stop by SIGINT/SIGTERM (not a failure of the recording).
    ///
    /// Only returned when the ordered finalization actually succeeded — the
    /// recorder finish/flush and the device release (ungrab and close) — so
    /// the "trace flushed and device released" contract is truthful (M5
    /// review R3). If any finalization step failed, [`CommandFailure::RecorderFinalize`]
    /// or [`CommandFailure::DeviceRelease`] is returned instead, preserving
    /// every diagnostic.
    #[error("stopped by SIGINT/SIGTERM; trace flushed and device released")]
    Stopped,
    /// The record finalization failed: the recorder could not be flushed /
    /// finished before the device release (exit code [`ExitCode::Recorder`]).
    /// The message preserves the primary stop reason and any device-release
    /// failures (M5 review R3).
    #[error("{0}")]
    RecorderFinalize(String),
    /// The device release failed during the ordered finalization — a failed
    /// ungrab and/or close (exit code [`ExitCode::Stream`]). The message
    /// preserves the primary stop reason and every release diagnostic (M5
    /// review R3).
    #[error("{0}")]
    DeviceRelease(String),
    /// The output backend is unavailable — no D-Bus session bus or no
    /// RemoteDesktop portal (output-probe, exit code [`ExitCode::InputDir`]).
    #[error("output backend unavailable: {0}")]
    OutputUnavailable(String),
    /// The authorization was cancelled or refused by the user/portal
    /// (output-probe, exit code [`ExitCode::Permission`]).
    #[error("output authorization: {0}")]
    OutputDenied(String),
    /// The libei library, the portal protocol version, or a required
    /// capability is missing (output-probe, exit code
    /// [`ExitCode::NoCandidate`]).
    #[error("output capability: {0}")]
    OutputCapability(String),
    /// The transport disconnected or the session timed out (output-probe,
    /// exit code [`ExitCode::Trace`]).
    #[error("output transport: {0}")]
    OutputDisconnected(String),
    /// A send failed (partial send failure; output-probe, exit code
    /// [`ExitCode::Stream`]).
    #[error("output send failed: {0}")]
    OutputSendFailed(String),
    /// Releasing held button/key/scroll state failed (output-probe, exit
    /// code [`ExitCode::Recorder`]).
    #[error("output release failed: {0}")]
    OutputReleaseFailed(String),
    /// Aborted by the user before/during emission; nothing was left held
    /// (output-probe, exit code [`ExitCode::Stopped`]).
    #[error("output aborted: {0}")]
    OutputCancelled(String),
    /// The M10 takeover was aborted by the user **before it began** (countdown
    /// cancel or a signal during the countdown, exit code
    /// [`ExitCode::Stopped`]): nothing was grabbed and no desktop input was
    /// emitted; the prepared output session was released, the recorder
    /// finalized, and the device closed. Cleanup failures are preserved in
    /// the message.
    #[error("takeover aborted before it began: {0}")]
    TakeoverAborted(String),
    /// Unexpected/internal error (including output write failures).
    #[error("internal error: {0}")]
    Unexpected(String),
    /// Runtime configuration could not be read, decoded, migrated, or
    /// semantically validated (M16 config-check/service-preflight).
    #[error("configuration error: {0}")]
    Config(String),
}

impl CommandFailure {
    /// The stable exit code for this failure.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        match self {
            CommandFailure::Usage(_) => ExitCode::Usage,
            CommandFailure::InputDir(_) => ExitCode::InputDir,
            CommandFailure::Permission(_) => ExitCode::Permission,
            CommandFailure::NoCandidate(_) => ExitCode::NoCandidate,
            CommandFailure::Trace(_) | CommandFailure::Replay(_) => ExitCode::Trace,
            CommandFailure::Config(_) => ExitCode::Usage,
            CommandFailure::Stream(_) => ExitCode::Stream,
            CommandFailure::Recorder(_) => ExitCode::Recorder,
            CommandFailure::Stopped => ExitCode::Stopped,
            CommandFailure::RecorderFinalize(_) => ExitCode::Recorder,
            CommandFailure::DeviceRelease(_) => ExitCode::Stream,
            CommandFailure::OutputUnavailable(_) => ExitCode::InputDir,
            CommandFailure::OutputDenied(_) => ExitCode::Permission,
            CommandFailure::OutputCapability(_) => ExitCode::NoCandidate,
            CommandFailure::OutputDisconnected(_) => ExitCode::Trace,
            CommandFailure::OutputSendFailed(_) => ExitCode::Stream,
            CommandFailure::OutputReleaseFailed(_) => ExitCode::Recorder,
            CommandFailure::OutputCancelled(_) => ExitCode::Stopped,
            CommandFailure::TakeoverAborted(_) => ExitCode::Stopped,
            CommandFailure::Unexpected(_) => ExitCode::Unexpected,
        }
    }
}
