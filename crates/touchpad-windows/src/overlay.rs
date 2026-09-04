//! Safe user-mode three-finger gesture overlay for Windows.
//!
//! Windows still owns ordinary Precision Touchpad pointer/scroll processing.
//! This adapter therefore does not feed one- or two-finger sequences into the
//! shared arbiter at all. A sequence is admitted only once exactly three live
//! contacts exist, and remains owned until the cluster is empty. This lets an
//! old Windows host test three-finger drag and three-finger middle-click
//! without doubling normal pointer or scroll output.

#![forbid(unsafe_code)]

use touchpad_core::{
    ArbiterConfig, ArbiterSink, ContactFrame, ContactState, GestureMapConfig, M19Profile,
    OutputSink, UserSettings,
};

use crate::WindowsError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OverlayState {
    #[default]
    Idle,
    Active,
    Drain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OverlayOutcome {
    pub forwarded: bool,
    pub emitted_events: usize,
}

pub(crate) struct ThreeFingerOverlay<S: OutputSink> {
    config: ArbiterConfig,
    pipeline: Option<ArbiterSink<S>>,
    state: OverlayState,
}

impl<S: OutputSink> ThreeFingerOverlay<S> {
    pub(crate) fn new(sink: S) -> Result<Self, WindowsError> {
        let full = M19Profile::new(UserSettings::default())
            .map_err(|error| WindowsError::Pipeline(error.to_string()))?
            .arbiter_config()
            .map_err(|error| WindowsError::Pipeline(error.to_string()))?;
        let drag = full
            .three_finger_drag_config()
            .cloned()
            .ok_or_else(|| WindowsError::Pipeline("M19 has no three-finger drag config".into()))?;
        let mut config =
            ArbiterConfig::new(full.motion_threshold_mm(), full.logical_pixels_per_mm())
                .map_err(|error| WindowsError::Pipeline(error.to_string()))?
                .with_three_finger_drag(drag)
                .with_gesture_bindings(GestureMapConfig::default());
        if let Some(fidelity) = full.three_finger_drag_fidelity_config().cloned() {
            config = config.with_three_finger_drag_fidelity(fidelity);
        }
        Ok(Self {
            pipeline: Some(ArbiterSink::new(config.clone(), sink)),
            config,
            state: OverlayState::Idle,
        })
    }

    pub(crate) fn frame(&mut self, frame: &ContactFrame) -> Result<OverlayOutcome, WindowsError> {
        let live = frame
            .contacts
            .iter()
            .filter(|contact| contact.state != ContactState::Ended)
            .count();

        match self.state {
            OverlayState::Idle => {
                if live >= 4 {
                    self.state = OverlayState::Drain;
                    return Ok(OverlayOutcome::default());
                }
                if live != 3
                    || frame
                        .contacts
                        .iter()
                        .any(|c| c.state == ContactState::Ended)
                {
                    return Ok(OverlayOutcome::default());
                }
                let mut admitted = frame.clone();
                for contact in &mut admitted.contacts {
                    if contact.state != ContactState::Ended {
                        contact.state = ContactState::Began;
                    }
                }
                self.state = OverlayState::Active;
                self.forward(&admitted)
            }
            OverlayState::Active => {
                if live >= 4 {
                    self.reset_pipeline()?;
                    self.state = OverlayState::Drain;
                    return Ok(OverlayOutcome::default());
                }
                let outcome = self.forward(frame)?;
                if live == 0 {
                    self.state = OverlayState::Idle;
                }
                Ok(outcome)
            }
            OverlayState::Drain => {
                if live == 0 {
                    self.state = OverlayState::Idle;
                }
                Ok(OverlayOutcome::default())
            }
        }
    }

    fn forward(&mut self, frame: &ContactFrame) -> Result<OverlayOutcome, WindowsError> {
        let decision = self
            .pipeline
            .as_mut()
            .expect("pipeline always installed")
            .frame(frame)
            .map_err(|error| WindowsError::Pipeline(error.to_string()))?;
        Ok(OverlayOutcome {
            forwarded: true,
            emitted_events: decision.events.len(),
        })
    }

    fn reset_pipeline(&mut self) -> Result<(), WindowsError> {
        let mut old = self.pipeline.take().expect("pipeline always installed");
        old.release_all()
            .map_err(|error| WindowsError::Pipeline(error.to_string()))?;
        let (_, sink) = old.into_parts();
        self.pipeline = Some(ArbiterSink::new(self.config.clone(), sink));
        Ok(())
    }

    pub(crate) fn release_all(&mut self) -> Result<(), WindowsError> {
        self.pipeline
            .as_mut()
            .expect("pipeline always installed")
            .release_all()
            .map_err(|error| WindowsError::Pipeline(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchpad_core::{Contact, Millimeters, Monotonic, RecordingSink};

    fn frame(sequence: u64, contacts: &[(i32, ContactState, f32)]) -> ContactFrame {
        let mut frame = ContactFrame::new(Monotonic::from_nanos(sequence * 10_000_000), sequence);
        frame.contacts = contacts
            .iter()
            .enumerate()
            .map(|(slot, (id, state, x))| {
                let mut contact = Contact::new(*id, slot as u32, *state);
                contact.x_mm = Some(Millimeters::try_new(*x).unwrap());
                contact.y_mm = Some(Millimeters::try_new(20.0).unwrap());
                contact
            })
            .collect();
        frame
    }

    #[test]
    fn ordinary_one_and_two_finger_input_is_never_forwarded() {
        let sink = RecordingSink::default();
        let mut overlay = ThreeFingerOverlay::new(sink).unwrap();
        assert!(
            !overlay
                .frame(&frame(1, &[(1, ContactState::Began, 10.0)]))
                .unwrap()
                .forwarded
        );
        assert!(
            !overlay
                .frame(&frame(
                    2,
                    &[
                        (1, ContactState::Active, 12.0),
                        (2, ContactState::Began, 20.0),
                    ],
                ))
                .unwrap()
                .forwarded
        );
    }

    #[test]
    fn staggered_three_finger_entry_is_rebased_as_new_cluster() {
        let sink = RecordingSink::default();
        let mut overlay = ThreeFingerOverlay::new(sink).unwrap();
        overlay
            .frame(&frame(1, &[(1, ContactState::Began, 10.0)]))
            .unwrap();
        overlay
            .frame(&frame(
                2,
                &[
                    (1, ContactState::Active, 10.0),
                    (2, ContactState::Began, 20.0),
                ],
            ))
            .unwrap();
        let admitted = overlay
            .frame(&frame(
                3,
                &[
                    (1, ContactState::Active, 10.0),
                    (2, ContactState::Active, 20.0),
                    (3, ContactState::Began, 30.0),
                ],
            ))
            .unwrap();
        assert!(admitted.forwarded);
    }

    #[test]
    fn four_finger_sequence_is_drained_without_output() {
        let sink = RecordingSink::default();
        let mut overlay = ThreeFingerOverlay::new(sink).unwrap();
        let four = frame(
            1,
            &[
                (1, ContactState::Began, 10.0),
                (2, ContactState::Began, 20.0),
                (3, ContactState::Began, 30.0),
                (4, ContactState::Began, 40.0),
            ],
        );
        assert!(!overlay.frame(&four).unwrap().forwarded);
        assert!(
            !overlay
                .frame(&frame(
                    2,
                    &[
                        (1, ContactState::Active, 10.0),
                        (2, ContactState::Active, 20.0),
                        (3, ContactState::Active, 30.0),
                    ],
                ))
                .unwrap()
                .forwarded
        );
    }

    #[test]
    fn release_all_is_safe_while_overlay_is_idle() {
        let sink = RecordingSink::default();
        let mut overlay = ThreeFingerOverlay::new(sink).unwrap();
        overlay.release_all().unwrap();
    }
}
