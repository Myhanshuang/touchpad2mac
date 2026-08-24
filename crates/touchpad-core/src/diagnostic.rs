//! Structured diagnostics attached to frames, descriptors, and conversion
//! failures.
//!
//! Diagnostics are the runtime's structured error surface: they never panic
//! and can be surfaced by the CLI, logs, or the offline replay tooling.

use serde::{Deserialize, Serialize};

/// Severity of a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DiagnosticLevel {
    /// Informational (e.g. recovered discontinuity).
    Info,
    /// Non-fatal anomaly (e.g. incomplete new contact).
    Warning,
    /// Violation of an invariant; the affected data must not be trusted.
    Error,
    /// The input stream cannot continue.
    Fatal,
}

/// Stable machine-readable code for a diagnostic.
///
/// `#[non_exhaustive]`: new codes are added as later milestones land without
/// breaking consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// An axis declares `min > max` (or an invalid fuzz/flat).
    InvalidAxisRange,
    /// An axis needed for millimeter conversion has no resolution and no
    /// profile override.
    MissingAxisResolution,
    /// A float value is `NaN` or infinite where it must be finite.
    NonFiniteValue,
    /// A value is outside its documented range (e.g. pressure outside
    /// `[0, 1]`).
    OutOfRangeValue,
    /// A new contact was published before both coordinates were known.
    IncompleteNewContact,
    /// Monotonic time went backwards.
    TimeRegression,
    /// A contact references a slot outside the device's slot range.
    SlotOutOfRange,
    /// A frame contains two contacts on the same slot.
    DuplicateSlot,
    /// Events arrived in an order the input state machine does not accept.
    InvalidEventOrder,
    /// The device cannot be handled by the runtime.
    UnsupportedDevice,
    /// The input stream lost continuity (e.g. `SYN_DROPPED`) and was
    /// recovered. Attached to the discontinuity frame published after a
    /// successful resynchronization (M3).
    DecodeRecovered,
    /// The input stream lost continuity and could not be recovered. The M3
    /// decoder surfaces that failure as a fatal decoder error (no frame is
    /// published on the failure path); this code remains reserved for later
    /// milestones' failure/grab-release notices.
    DecodeDegraded,
    /// A Type-B slot's tracking id was replaced while the slot was active;
    /// the previous contact ended implicitly. Emitted by the Type-B decoder
    /// (M3).
    TrackingIdReplaced,
    /// A new contact could not be published when it began because its
    /// coordinates were incomplete; it was published once they became
    /// complete. Emitted by the Type-B decoder (M3).
    DelayedNewContact,
    /// A one-finger pointer/tap-family interaction began as a candidate
    /// (M7 arbiter). Informational.
    InteractionBegun,
    /// A candidate crossed its configured motion threshold and committed to
    /// pointer output (M7 arbiter). Informational.
    InteractionCommitted,
    /// An interaction was cancelled (second live contact, discontinuity,
    /// missing required coordinates, or another arbiter cancel trigger);
    /// no further output is produced from it (M7 arbiter).
    InteractionCancelled,
    /// An interaction finished cleanly because its contact ended (M7
    /// arbiter). Informational.
    InteractionFinished,
    /// A qualifying one-finger tap emitted its click pair at the release
    /// frame (M8 arbiter). Informational.
    TapFired,
    /// A pending tap-and-drag follow-up contact committed pointer motion and
    /// began the synthetic held-left drag (M8 arbiter). Informational.
    TapAndDragBegan,
    /// The arbiter entered sticky drag lock: synthetic left stays held after
    /// a committed tap-drag lift (M8 arbiter). Informational.
    DragLocked,
    /// A qualifying tap while drag-locked released the synthetic left and
    /// left drag lock (M8 arbiter). Informational.
    DragUnlocked,
    /// A two-finger candidate began: the frame where the second valid
    /// contact appeared anchored a two-finger scroll/tap interaction (M9
    /// arbiter). Informational.
    TwoFingerScrollBegan,
    /// A two-finger scroll candidate crossed its configured threshold and
    /// committed: `ScrollBegin` plus the accepted accumulated centroid
    /// displacement were emitted (M9 arbiter). Informational.
    TwoFingerScrollCommitted,
    /// A committed two-finger scroll ended: `ScrollEnd` was emitted (M9
    /// arbiter). Informational.
    TwoFingerScrollEnded,
    /// A qualifying two-finger tap emitted its secondary (right) click pair
    /// at the release boundary (M9 arbiter). Informational.
    SecondaryTapFired,
    /// A two-finger interaction was cancelled (physical click, third finger,
    /// missing required coordinates, tracking-id replacement, discontinuity,
    /// or a deterministic cancel; M9 arbiter). No secondary tap and no
    /// further scroll output is produced from it.
    TwoFingerCancelled,
    /// A physical two-finger buttonpad press latched the press to the
    /// secondary (right) button for its whole duration (M9 arbiter).
    /// Informational.
    SecondaryClickLatched,
}

/// A single structured diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub code: DiagnosticCode,
    pub message: String,
    /// Frame sequence this diagnostic refers to, when known.
    pub frame_sequence: Option<u64>,
}

impl Diagnostic {
    /// Creates a diagnostic not tied to a specific frame.
    #[must_use]
    pub fn new(level: DiagnosticLevel, code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            level,
            code,
            message: message.into(),
            frame_sequence: None,
        }
    }

    /// Creates a diagnostic tied to a frame sequence.
    #[must_use]
    pub fn with_frame(
        level: DiagnosticLevel,
        code: DiagnosticCode,
        message: impl Into<String>,
        frame_sequence: u64,
    ) -> Self {
        Self {
            level,
            code,
            message: message.into(),
            frame_sequence: Some(frame_sequence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_construction() {
        let d = Diagnostic::new(
            DiagnosticLevel::Warning,
            DiagnosticCode::IncompleteNewContact,
            "incomplete contact",
        );
        assert_eq!(d.level, DiagnosticLevel::Warning);
        assert_eq!(d.code, DiagnosticCode::IncompleteNewContact);
        assert_eq!(d.frame_sequence, None);

        let d = Diagnostic::with_frame(
            DiagnosticLevel::Error,
            DiagnosticCode::DuplicateSlot,
            "duplicate slot",
            7,
        );
        assert_eq!(d.frame_sequence, Some(7));
    }
}
