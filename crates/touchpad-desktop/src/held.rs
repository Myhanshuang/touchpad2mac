#![forbid(unsafe_code)]
//! Held button/key/scroll state tracking and idempotent release (M6
//! required outcome 3).
//!
//! The adapter tracks every button/key press and the open smooth-scroll
//! lifecycle it has *successfully submitted* to the transport, so
//! [`release_all`](crate::sink::PortalOutputSink::release_all) can return the
//! session to a neutral state on every path — normal shutdown, fatal
//! shutdown, partial send failure, and fallback `Drop` — and is idempotent.
//!
//! A `ButtonUp`/`KeyUp` for a state that is not held and a
//! `ScrollDelta`/`ScrollEnd` without an open scroll lifecycle are rejected
//! as protocol misuse ([`touchpad_core::OutputError::Rejected`]); the tracked
//! state is the single source of truth for what must be released, so it can
//! never disagree with what was actually sent.

use std::collections::BTreeSet;

use touchpad_core::output::KeyId;
use touchpad_core::{MouseButton, OutputError, OutputEvent};

/// The tracked, logically-held output state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldState {
    buttons: Vec<MouseButton>,
    keys: BTreeSet<KeyId>,
    scroll_open: bool,
    /// Whether at least one **nonzero x delta** was sent in the current
    /// scroll interaction. libei tracks scrolling per axis, so
    /// `scroll_stop`/cancel must reflect exactly the axes that received
    /// nonzero deltas (M6 re-review R5).
    scroll_x_active: bool,
    /// Whether at least one **nonzero y delta** was sent in the current
    /// scroll interaction. See `scroll_x_active`.
    scroll_y_active: bool,
}

impl HeldState {
    /// Creates an empty (neutral) held state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pure lifecycle validation, without mutating any state. The adapter
    /// runs this *before* sending an event to the wire, so a lifecycle
    /// misuse (e.g. `ScrollEnd` without `ScrollBegin`) is rejected before
    /// any partial wire sequence is emitted.
    pub fn validate(&self, event: &OutputEvent) -> Result<(), OutputError> {
        match event {
            OutputEvent::ButtonUp(button) => {
                if self.buttons.contains(button) {
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::ScrollDelta { .. } => {
                if self.scroll_open {
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::ScrollEnd => {
                if self.scroll_open {
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::KeyUp(key) => {
                if self.keys.contains(key) {
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            _ => Ok(()),
        }
    }

    /// Records a successfully-submitted event, validating the lifecycle.
    ///
    /// * `ButtonDown` marks the button held (duplicates are no-ops).
    /// * `ButtonUp` requires the button to be held; otherwise the event is
    ///   rejected (a release for a button that was never pressed would make
    ///   the tracked state lie about the wire state).
    /// * `ScrollBegin` opens the scroll lifecycle (it has **no** libei wire
    ///   event: the first nonzero `ScrollDelta` starts scrolling on the
    ///   server side) and resets the per-axis activity (each interaction is
    ///   tracked separately).
    /// * `ScrollDelta` requires an open lifecycle and marks the axes that
    ///   received a **nonzero** delta (zero deltas never activate an axis),
    ///   so release stops exactly the active axes.
    /// * `ScrollEnd` requires an open lifecycle, closes it, and resets the
    ///   per-axis activity.
    /// * `KeyDown`/`KeyUp` mirror the button rules.
    ///
    /// Pointer motion and desktop actions carry no held state.
    pub fn record(&mut self, event: &OutputEvent) -> Result<(), OutputError> {
        match event {
            OutputEvent::ButtonDown(button) => {
                if !self.buttons.contains(button) {
                    self.buttons.push(*button);
                }
                Ok(())
            }
            OutputEvent::ButtonUp(button) => {
                if let Some(index) = self.buttons.iter().position(|held| held == button) {
                    self.buttons.swap_remove(index);
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::ScrollBegin => {
                self.scroll_open = true;
                self.scroll_x_active = false;
                self.scroll_y_active = false;
                Ok(())
            }
            OutputEvent::ScrollDelta { dx, dy } => {
                if self.scroll_open {
                    if dx.as_px() != 0.0 {
                        self.scroll_x_active = true;
                    }
                    if dy.as_px() != 0.0 {
                        self.scroll_y_active = true;
                    }
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::ScrollEnd => {
                if self.scroll_open {
                    self.scroll_open = false;
                    self.scroll_x_active = false;
                    self.scroll_y_active = false;
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::KeyDown(key) => {
                self.keys.insert(*key);
                Ok(())
            }
            OutputEvent::KeyUp(key) => {
                if self.keys.remove(key) {
                    Ok(())
                } else {
                    Err(OutputError::Rejected(event.clone()))
                }
            }
            OutputEvent::PointerMove { .. }
            | OutputEvent::DesktopAction(_)
            | OutputEvent::ContinuousGesture(_) => Ok(()),
        }
    }

    /// The axes that received at least one nonzero delta in the current
    /// scroll interaction, as the `(stop_x, stop_y)` pair for
    /// `ei_device_scroll_stop` (M6 re-review R5: stop only active axes).
    /// Meaningful only while a scroll lifecycle is open.
    #[must_use]
    pub fn scroll_stop_axes(&self) -> (bool, bool) {
        (self.scroll_x_active, self.scroll_y_active)
    }

    /// The release events that return the tracked state to neutral: one
    /// `ButtonUp` per held button, `ScrollEnd` when a scroll lifecycle is
    /// open **and** at least one axis received a nonzero delta (a bare
    /// `ScrollStop` with no delta is documented by libei as a client logic
    /// bug), one `KeyUp` per held key. Deterministic order (buttons in
    /// insertion order, then scroll, then keys sorted by id).
    #[must_use]
    pub fn release_events(&self) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        events.extend(self.buttons.iter().copied().map(OutputEvent::ButtonUp));
        if self.scroll_open && (self.scroll_x_active || self.scroll_y_active) {
            events.push(OutputEvent::ScrollEnd);
        }
        events.extend(self.keys.iter().copied().map(OutputEvent::KeyUp));
        events
    }

    /// Whether the tracked state is neutral (nothing held, no open scroll
    /// lifecycle).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.buttons.is_empty() && self.keys.is_empty() && !self.scroll_open
    }

    /// Clears all tracked state (used after the release attempt, whose
    /// failures are reported separately).
    pub fn clear(&mut self) {
        self.buttons.clear();
        self.keys.clear();
        self.scroll_open = false;
        self.scroll_x_active = false;
        self.scroll_y_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchpad_core::{LogicalPixels, MouseButton};

    fn move_event() -> OutputEvent {
        OutputEvent::PointerMove {
            dx: LogicalPixels::try_new(1.0).unwrap(),
            dy: LogicalPixels::try_new(0.0).unwrap(),
        }
    }

    fn delta(dx: f32, dy: f32) -> OutputEvent {
        OutputEvent::ScrollDelta {
            dx: LogicalPixels::try_new(dx).unwrap(),
            dy: LogicalPixels::try_new(dy).unwrap(),
        }
    }

    fn delta_y(dy: f32) -> OutputEvent {
        delta(0.0, dy)
    }

    #[test]
    fn button_lifecycle_is_tracked_and_released() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        held.record(&OutputEvent::ButtonDown(MouseButton::Right))
            .unwrap();
        assert!(!held.is_clean());
        let events = held.release_events();
        assert_eq!(
            events,
            vec![
                OutputEvent::ButtonUp(MouseButton::Left),
                OutputEvent::ButtonUp(MouseButton::Right),
            ]
        );
        for event in &events {
            held.record(event).unwrap();
        }
        assert!(held.is_clean());
        // Releasing an already-neutral state emits nothing.
        assert!(held.release_events().is_empty());
    }

    #[test]
    fn duplicate_button_down_is_a_no_op() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        held.record(&OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        assert_eq!(held.release_events().len(), 1);
    }

    #[test]
    fn button_up_without_down_is_rejected() {
        let mut held = HeldState::new();
        let err = held.record(&OutputEvent::ButtonUp(MouseButton::Left));
        assert!(matches!(
            err,
            Err(OutputError::Rejected(OutputEvent::ButtonUp(
                MouseButton::Left
            )))
        ));
        assert!(held.is_clean());
    }

    #[test]
    fn scroll_lifecycle_requires_begin_and_delta_before_release_stop() {
        let mut held = HeldState::new();
        // A delta without begin is rejected.
        assert!(held.record(&delta_y(-10.0)).is_err());
        // An end without begin is rejected.
        assert!(held.record(&OutputEvent::ScrollEnd).is_err());

        // begin + delta -> release must include ScrollEnd (a delta was sent).
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta_y(-10.0)).unwrap();
        assert_eq!(held.release_events(), vec![OutputEvent::ScrollEnd]);

        // begin without any nonzero delta -> nothing was scrolled; release
        // emits no bare ScrollEnd (libei documents scroll-stop-without-delta
        // as a client logic bug).
        let mut empty = HeldState::new();
        empty.record(&OutputEvent::ScrollBegin).unwrap();
        assert!(empty.release_events().is_empty());
    }

    /// M6 re-review R5: a y-only interaction (like the fixed `--emit` probe,
    /// which scrolls `(0, -120)` and `(0, -240)`) must stop only the y axis.
    #[test]
    fn y_only_scroll_stops_only_y() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta_y(-120.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, true));
        assert_eq!(held.release_events(), vec![OutputEvent::ScrollEnd]);
        // After recording the ScrollEnd the per-axis state resets.
        held.record(&OutputEvent::ScrollEnd).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, false));
        assert!(held.is_clean());
    }

    /// M6 re-review R5: an x-only interaction stops only the x axis.
    #[test]
    fn x_only_scroll_stops_only_x() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta(-10.0, 0.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (true, false));
        assert_eq!(held.release_events(), vec![OutputEvent::ScrollEnd]);
    }

    /// M6 re-review R5: a two-axis interaction stops both axes.
    #[test]
    fn two_axis_scroll_stops_both_axes() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta(-10.0, -20.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (true, true));
        assert_eq!(held.release_events(), vec![OutputEvent::ScrollEnd]);
    }

    /// M6 re-review R5: a zero delta never activates an axis, so the
    /// lifecycle is not stopped at all on release (nothing was scrolled).
    #[test]
    fn zero_delta_activates_no_axis() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta(0.0, 0.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, false));
        assert!(held.release_events().is_empty());
        // A later nonzero delta still activates only its axis.
        held.record(&delta_y(-5.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, true));
        assert_eq!(held.release_events(), vec![OutputEvent::ScrollEnd]);
    }

    /// M6 re-review R5: each scroll interaction is tracked separately — a
    /// previous interaction's axes do not leak into the next one.
    #[test]
    fn repeated_scroll_lifecycles_reset_axis_state() {
        let mut held = HeldState::new();
        // Interaction 1: x-only.
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta(-10.0, 0.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (true, false));
        held.record(&OutputEvent::ScrollEnd).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, false));
        // Interaction 2: y-only — x must not be stopped.
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta_y(-30.0)).unwrap();
        assert_eq!(held.scroll_stop_axes(), (false, true));
        held.record(&OutputEvent::ScrollEnd).unwrap();
        assert!(held.is_clean());
    }

    /// M6 re-review R5: a mix of nonzero and zero deltas accumulates the
    /// active axes per axis.
    #[test]
    fn partial_axis_activity_accumulates_independently() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.record(&delta(-10.0, 0.0)).unwrap(); // x active
        held.record(&delta(0.0, 0.0)).unwrap(); // no change
        held.record(&delta(0.0, -20.0)).unwrap(); // y active
        assert_eq!(held.scroll_stop_axes(), (true, true));
    }

    #[test]
    fn keys_follow_the_button_rules() {
        let mut held = HeldState::new();
        let key = KeyId::new(42);
        held.record(&OutputEvent::KeyDown(key)).unwrap();
        assert_eq!(held.release_events(), vec![OutputEvent::KeyUp(key)]);
        assert!(held.record(&OutputEvent::KeyUp(KeyId::new(43))).is_err());
    }

    #[test]
    fn motion_and_actions_carry_no_state() {
        let mut held = HeldState::new();
        held.record(&move_event()).unwrap();
        held.record(&OutputEvent::DesktopAction(
            touchpad_core::DesktopAction::ShowDesktop,
        ))
        .unwrap();
        assert!(held.is_clean());
    }

    #[test]
    fn clear_resets_everything() {
        let mut held = HeldState::new();
        held.record(&OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        held.record(&OutputEvent::ScrollBegin).unwrap();
        held.clear();
        assert!(held.is_clean());
        assert!(held.release_events().is_empty());
    }
}
