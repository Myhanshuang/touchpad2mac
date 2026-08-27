//! Typed semantic output contract (design.md §13, IMPLEMENTATION_BRIEF §9).
//!
//! This milestone defines the *contract only*: no Wayland, X11, or uinput
//! backend is implemented here. [`RecordingSink`] records events for tests;
//! real backends are later milestones and must be separately validated
//! against the invariants in IMPLEMENTATION_BRIEF §9 before they can become
//! defaults.

use serde::{Deserialize, Serialize};

use crate::time::Monotonic;
use crate::units::LogicalPixels;

/// Mouse buttons in the semantic output model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// Any other button, identified by a platform-assigned code.
    Other(u8),
}

/// Opaque platform-assigned key identifier for `KeyDown`/`KeyUp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KeyId(u32);

impl KeyId {
    /// Creates a key id.
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// The raw id value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

/// Desktop-level actions a gesture can trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DesktopAction {
    NextWorkspace,
    PreviousWorkspace,
    ShowDesktop,
    OpenOverview,
    CloseOverview,
    PresentWindows,
    ApplicationLauncher,
    NotificationCenter,
    PageNext,
    PagePrevious,
    SmartZoom,
    Lookup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContinuousGestureKind {
    Pinch,
    Rotate,
    TwoFingerPageSwipe,
    ThreeFingerSwipe,
    FourFingerSwipe,
    EdgeSwipe,
    ThumbThreePinch,
    ThumbThreeSpread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContinuousGesturePhase {
    Begin,
    Update,
    End,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContinuousGestureEvent {
    pub kind: ContinuousGestureKind,
    pub phase: ContinuousGesturePhase,
    pub translation_x_mm: f32,
    pub translation_y_mm: f32,
    pub scale: f32,
    pub rotation_radians: f32,
}

/// One resolved semantic output event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OutputEvent {
    /// Move the pointer by a logical delta.
    PointerMove {
        dx: LogicalPixels,
        dy: LogicalPixels,
    },
    ButtonDown(MouseButton),
    ButtonUp(MouseButton),
    /// Start of a smooth scroll interaction.
    ScrollBegin,
    /// A smooth scroll delta.
    ScrollDelta {
        dx: LogicalPixels,
        dy: LogicalPixels,
    },
    /// End of a smooth scroll interaction.
    ScrollEnd,
    KeyDown(KeyId),
    KeyUp(KeyId),
    DesktopAction(DesktopAction),
    ContinuousGesture(ContinuousGestureEvent),
}

/// Failure modes of an output backend.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OutputError {
    /// The backend cannot be used (e.g. not implemented in this milestone).
    #[error("output backend is not available: {0}")]
    Unavailable(String),
    /// An I/O-level failure in the backend.
    #[error("output backend I/O failure: {0}")]
    Io(String),
    /// The backend rejected a specific event.
    #[error("output backend rejected event {0:?}")]
    Rejected(OutputEvent),
    /// The backend failed in a way that requires releasing held button/key
    /// state before continuing or shutting down.
    #[error("output backend failed; held button/key state must be released: {0}")]
    Fatal(String),
}

/// Failure while submitting one logical input frame worth of semantic
/// output. `accepted_prefix` is the number of leading semantic events whose
/// delivery is known to have completed; `failed_index` identifies the event
/// whose wire operation exposed the failure. They are usually equal for the
/// default event-by-event sink, but a backend that batches several semantic
/// events into one protocol frame may have accepted none of that batch when
/// the shared frame commit fails.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error(
    "output frame failed at event {failed_index} after accepting {accepted_prefix} event(s): {primary}"
)]
pub struct OutputFrameError {
    pub failed_index: usize,
    pub accepted_prefix: usize,
    pub primary: OutputError,
}

/// Contract for emitting resolved semantic output.
///
/// * **Ordering** — events must be observed in the order they are submitted.
/// * **Backpressure** — sinks are synchronous: `submit` blocks until the
///   event is accepted, and reports failure via [`OutputError`].
/// * **Partial failure** — a failed `submit` must leave the sink in a state
///   where a retry or shutdown (`release_all`) is still well-defined.
/// * **Shutdown** — callers must invoke [`release_all`](Self::release_all)
///   before dropping a live sink so held button/key state is released.
pub trait OutputSink {
    /// Submits one resolved output event.
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError>;

    /// Submits all semantic events produced by one logical input frame.
    ///
    /// The default implementation preserves the historical synchronous
    /// event-by-event contract. Protocol backends with an explicit hardware
    /// frame boundary (notably libei) may override this method to keep a
    /// button edge and its owned pointer motion in the same protocol frame.
    /// Such an override must preserve semantic ordering and report exactly
    /// how much of the leading semantic prefix is known to have committed.
    fn submit_frame(&mut self, events: &[OutputEvent]) -> Result<(), OutputFrameError> {
        for (index, event) in events.iter().enumerate() {
            if let Err(primary) = self.submit(event.clone()) {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary,
                });
            }
        }
        Ok(())
    }

    /// Submits one logical input frame together with the source monotonic
    /// timestamp. Protocol backends that can timestamp their own hardware
    /// frames should override this method. Legacy sinks deliberately fall
    /// back to [`Self::submit_frame`] so adding timestamp propagation does not
    /// change their delivery semantics.
    fn submit_frame_at(
        &mut self,
        _timestamp: Monotonic,
        events: &[OutputEvent],
    ) -> Result<(), OutputFrameError> {
        self.submit_frame(events)
    }

    /// Releases all held button/key state. Must be idempotent.
    fn release_all(&mut self) -> Result<(), OutputError>;
}

/// A test sink that records every submitted event in order.
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    events: Vec<OutputEvent>,
}

impl RecordingSink {
    /// Creates an empty recording sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The events recorded so far, in submission order.
    #[must_use]
    pub fn events(&self) -> &[OutputEvent] {
        &self.events
    }

    /// Takes the recorded events, leaving the sink empty.
    #[must_use]
    pub fn take_events(&mut self) -> Vec<OutputEvent> {
        std::mem::take(&mut self.events)
    }

    /// Whether no events have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// How many events have been recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

impl OutputSink for RecordingSink {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        self.events.push(event);
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        Ok(()) // recording only; holds no button/key state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn move_event(dx: f32, dy: f32) -> OutputEvent {
        OutputEvent::PointerMove {
            dx: LogicalPixels::try_new(dx).unwrap(),
            dy: LogicalPixels::try_new(dy).unwrap(),
        }
    }

    #[test]
    fn recording_sink_preserves_order() {
        let mut sink = RecordingSink::new();
        sink.submit(move_event(1.0, 2.0)).unwrap();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        assert_eq!(sink.len(), 3);
        let events = sink.take_events();
        assert_eq!(events[0], move_event(1.0, 2.0));
        assert_eq!(events[1], OutputEvent::ButtonDown(MouseButton::Left));
        assert_eq!(events[2], OutputEvent::ScrollBegin);
        assert!(sink.is_empty());
    }

    struct FailingSink {
        submitted: usize,
        releases: usize,
    }

    impl OutputSink for FailingSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            self.submitted += 1;
            if self.submitted == 1 {
                Err(OutputError::Rejected(event))
            } else {
                Ok(())
            }
        }

        fn release_all(&mut self) -> Result<(), OutputError> {
            self.releases += 1;
            Ok(())
        }
    }

    #[test]
    fn partial_failure_is_reported_and_shutdown_is_well_defined() {
        let mut sink = FailingSink {
            submitted: 0,
            releases: 0,
        };
        assert_eq!(
            sink.submit(move_event(0.0, 0.0)),
            Err(OutputError::Rejected(move_event(0.0, 0.0)))
        );
        // A retry after a partial failure still works.
        sink.submit(move_event(1.0, 1.0)).unwrap();
        sink.release_all().unwrap();
        assert_eq!(sink.submitted, 2);
        assert_eq!(sink.releases, 1);
    }

    #[test]
    fn output_events_round_trip_through_json() {
        let event = OutputEvent::ScrollDelta {
            dx: LogicalPixels::try_new(0.0).unwrap(),
            dy: LogicalPixels::try_new(-5.5).unwrap(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<OutputEvent>(&json).unwrap(), event);
    }
}
