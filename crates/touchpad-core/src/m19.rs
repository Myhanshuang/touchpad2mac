//! M19 live-settings profile.
//!
//! M19 inherits M18's user settings and gesture mapping, with two live-use
//! tap-and-drag refinements: a completed tap arms the immediately-following
//! contact for drag for a short libinput-aligned window, and committed
//! tap-and-drag does not use sticky drag lock.
//! A committed drag therefore releases synthetic left on the clean contact
//! Ended frame instead of carrying held-left ownership into the next action.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{
    ArbiterConfig, FidelityConfig, FidelityConfigError, M18Profile, M18ProfileError,
    TapConfigError, UserSettings,
};

pub const M19_LIVE_V1_NAME: &str = "m19-live-v1";
const M19_THREE_FINGER_DRAG_MAX_GAIN: f64 = 1.6;
const M19_TAP_DRAG_GAP: Duration = Duration::from_millis(180);
const M19_THREE_FINGER_ENTRY_DEBOUNCE: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum M19ProfileError {
    #[error("invalid M18 base profile: {0}")]
    M18(M18ProfileError),
    #[error("invalid M19 tap timing refinement: {0}")]
    Tap(TapConfigError),
    #[error("invalid M19 three-finger drag fidelity refinement: {0}")]
    Fidelity(FidelityConfigError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct M19Profile {
    base: M18Profile,
}

impl M19Profile {
    pub const NAME: &str = M19_LIVE_V1_NAME;

    pub fn new(settings: UserSettings) -> Result<Self, M19ProfileError> {
        Ok(Self {
            base: M18Profile::new(settings).map_err(M19ProfileError::M18)?,
        })
    }

    pub fn arbiter_config(&self) -> Result<ArbiterConfig, M19ProfileError> {
        let config = self.base.arbiter_config().map_err(M19ProfileError::M18)?;
        let drag_fidelity = config
            .fidelity_config()
            .map(|pointer| {
                // Three-finger drag drives a compositor-rendered drag item as
                // well as the hardware cursor. Cap only its high-speed gain
                // so a 165 Hz touch stream cannot move the cursor tens of
                // pixels multiple times inside one 120 Hz scene refresh.
                // Low-speed gain/tracking speed stay identical to the user's
                // pointer feel; only the high-speed ceiling is reduced.
                let max_gain = pointer
                    .max_gain()
                    .min(M19_THREE_FINGER_DRAG_MAX_GAIN)
                    .max(pointer.min_gain());
                FidelityConfig::new(
                    pointer.dead_zone_radius_mm(),
                    pointer.velocity_tau(),
                    pointer.long_gap(),
                    pointer.gain_x0_mm_per_s(),
                    pointer.gain_x1_mm_per_s(),
                    pointer.min_gain(),
                    max_gain,
                    pointer.base_px_per_mm(),
                    pointer.tracking_speed(),
                )
                .map_err(M19ProfileError::Fidelity)
            })
            .transpose()?;
        let tap = config
            .tap_config()
            .cloned()
            .map(|tap| {
                tap.without_drag_lock()
                    .with_double_tap_before_drag(false)
                    .with_max_tap_drag_gap(M19_TAP_DRAG_GAP)
                    .map_err(M19ProfileError::Tap)
            })
            .transpose()?;
        let drag = config.three_finger_drag_config().cloned().map(|drag| {
            drag.with_stable_reference_motion(true)
                .with_entry_debounce(M19_THREE_FINGER_ENTRY_DEBOUNCE)
        });
        let config = match tap {
            Some(tap) => config.with_tap(tap),
            None => config,
        };
        let config = match drag {
            Some(drag) => config.with_three_finger_drag(drag),
            None => config,
        };
        Ok(match drag_fidelity {
            Some(fidelity) => config.with_three_finger_drag_fidelity(fidelity),
            None => config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Arbiter, Contact, ContactFrame, ContactState, Millimeters, Monotonic, MouseButton,
        OutputEvent,
    };

    fn contact(id: i32, state: ContactState, x: f32, y: f32) -> Contact {
        let mut contact = Contact::new(id, 0, state);
        contact.x_mm = Some(Millimeters::try_new(x).unwrap());
        contact.y_mm = Some(Millimeters::try_new(y).unwrap());
        contact
    }

    fn drag_contact(id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
        let mut contact = Contact::new(id, slot, state);
        contact.x_mm = Some(Millimeters::try_new(x).unwrap());
        contact.y_mm = Some(Millimeters::try_new(y).unwrap());
        contact
    }

    fn three(state: ContactState, x: f32) -> Vec<Contact> {
        vec![
            drag_contact(10, 0, state, x, 10.0),
            drag_contact(11, 1, state, x + 5.0, 10.0),
            drag_contact(12, 2, state, x + 10.0, 10.0),
        ]
    }

    fn frame(sequence: u64, nanos: u64, contacts: Vec<Contact>) -> ContactFrame {
        let mut frame = ContactFrame::new(Monotonic::from_nanos(nanos), sequence);
        frame.contacts = contacts;
        frame
    }

    #[test]
    fn m19_refines_one_finger_tap_drag_to_libinput_aligned_short_follow_up() {
        let mut settings = UserSettings::default();
        settings
            .set_key("feel.pointer.tracking_speed", "1.25")
            .unwrap();
        settings.set_key("feel.pointer.max_gain", "2.90").unwrap();
        let m18 = M18Profile::new(settings.clone())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let m19 = M19Profile::new(settings).unwrap().arbiter_config().unwrap();

        let m18_tap = m18.tap_config().expect("M18 inherits the M10 tap policy");
        let m19_tap = m19.tap_config().expect("M19 keeps tap-to-click enabled");
        assert!(m18_tap.drag_lock_enabled());
        assert!(!m19_tap.drag_lock_enabled());
        assert!(!m18_tap.double_tap_before_drag());
        assert!(!m19_tap.double_tap_before_drag());
        assert_eq!(m19_tap.tap_enabled(), m18_tap.tap_enabled());
        assert_eq!(
            m19_tap.tap_and_drag_enabled(),
            m18_tap.tap_and_drag_enabled()
        );
        assert_eq!(m19_tap.max_tap_duration(), m18_tap.max_tap_duration());
        assert_eq!(m19_tap.max_tap_movement_mm(), m18_tap.max_tap_movement_mm());
        assert_eq!(m18_tap.max_tap_drag_gap(), Duration::from_millis(350));
        assert_eq!(m19_tap.max_tap_drag_gap(), M19_TAP_DRAG_GAP);

        let pointer = m19.fidelity_config().expect("M19 keeps pointer fidelity");
        let drag = m19
            .three_finger_drag_fidelity_config()
            .expect("M19 installs a drag-only fidelity profile");
        assert_eq!(pointer.tracking_speed(), 1.25);
        assert_eq!(pointer.max_gain(), 2.9);
        assert_eq!(drag.min_gain(), pointer.min_gain());
        assert_eq!(drag.tracking_speed(), pointer.tracking_speed());
        assert_eq!(drag.max_gain(), M19_THREE_FINGER_DRAG_MAX_GAIN);

        let m18_drag = m18
            .three_finger_drag_config()
            .expect("M18 has the M15 drag recognizer");
        let m19_drag = m19
            .three_finger_drag_config()
            .expect("M19 keeps the M15 drag recognizer");
        assert!(!m18_drag.stable_reference_motion());
        assert!(m19_drag.stable_reference_motion());
    }

    #[test]
    fn m19_three_finger_commit_discards_classifier_motion_and_defers_press() {
        let config = M19Profile::new(UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut arbiter = Arbiter::new(config);

        arbiter
            .frame(&frame(0, 0, three(ContactState::Began, 0.0)))
            .unwrap();

        // Close the M19 entry-debounce window. All touchdown/settling motion
        // before this point is discarded. A stable fast-entered three-finger
        // cluster arms immediately here and uses this frame as the reference
        // baseline; it still does not press left or move the cursor.
        let armed = arbiter
            .frame(&frame(1, 50_000_000, three(ContactState::Active, 0.4)))
            .unwrap();
        assert!(!armed.events.iter().any(|event| matches!(
            event,
            OutputEvent::ButtonDown(MouseButton::Left) | OutputEvent::PointerMove { .. }
        )));
        assert!(!arbiter.is_synthetic_left_held());

        // The first post-arm reference delta is real drag motion. There is no
        // second 0.8 mm classifier after the debounce: that extra threshold
        // delayed fast flicks into their high-speed phase. The first emitted
        // motion establishes left ownership in the same semantic decision.
        let first = arbiter
            .frame(&frame(2, 60_000_000, three(ContactState::Active, 1.6)))
            .unwrap();
        let second = arbiter
            .frame(&frame(3, 70_000_000, three(ContactState::Active, 4.0)))
            .unwrap();
        let moved = if first
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::PointerMove { .. }))
        {
            first
        } else {
            second
        };
        let down = moved
            .events
            .iter()
            .position(|event| matches!(event, OutputEvent::ButtonDown(MouseButton::Left)))
            .expect("first emitted drag motion must establish left ownership");
        let motion = moved
            .events
            .iter()
            .position(|event| matches!(event, OutputEvent::PointerMove { .. }))
            .expect("post-commit reference motion must move the pointer");
        assert!(
            down < motion,
            "ButtonDown must precede the first PointerMove"
        );
        assert!(arbiter.is_synthetic_left_held());
    }

    #[test]
    fn m19_single_tap_then_follow_up_motion_drags_and_releases_on_lift() {
        let config = M19Profile::new(UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut arbiter = Arbiter::new(config);

        // First tap opens the follow-up window.
        arbiter
            .frame(&frame(
                0,
                0,
                vec![contact(1, ContactState::Began, 0.0, 0.0)],
            ))
            .unwrap();
        arbiter
            .frame(&frame(
                1,
                100_000_000,
                vec![contact(1, ContactState::Ended, 0.1, 0.0)],
            ))
            .unwrap();

        // The follow-up begins 70 ms after the tap release, comfortably
        // inside the M19/libinput-aligned 180 ms drag-arm window.
        arbiter
            .frame(&frame(
                2,
                170_000_000,
                vec![contact(2, ContactState::Began, 10.0, 10.0)],
            ))
            .unwrap();
        let committed = arbiter
            .frame(&frame(
                3,
                190_000_000,
                vec![contact(2, ContactState::Active, 12.0, 10.0)],
            ))
            .unwrap();
        assert!(committed
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::ButtonDown(MouseButton::Left))));
        assert!(committed
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::PointerMove { .. })));
        assert!(arbiter.is_synthetic_left_held());

        let ended = arbiter
            .frame(&frame(
                4,
                210_000_000,
                vec![contact(2, ContactState::Ended, 12.0, 10.0)],
            ))
            .unwrap();
        assert!(ended
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::ButtonUp(MouseButton::Left))));
        assert!(!arbiter.is_synthetic_left_held());
        assert!(!arbiter.is_left_held());
    }

    #[test]
    fn m19_tap_drag_arm_expires_strictly_after_180_ms() {
        let config = M19Profile::new(UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut arbiter = Arbiter::new(config);

        // One complete tap.
        arbiter
            .frame(&frame(
                0,
                0,
                vec![contact(1, ContactState::Began, 0.0, 0.0)],
            ))
            .unwrap();
        arbiter
            .frame(&frame(
                1,
                80_000_000,
                vec![contact(1, ContactState::Ended, 0.1, 0.0)],
            ))
            .unwrap();

        // A new contact starts 181 ms after release. The short drag arm has
        // expired, so crossing the pointer threshold is ordinary motion and
        // must not synthesize a held-left drag.
        arbiter
            .frame(&frame(
                2,
                261_000_000,
                vec![contact(2, ContactState::Began, 10.0, 10.0)],
            ))
            .unwrap();
        let moved = arbiter
            .frame(&frame(
                3,
                281_000_000,
                vec![contact(2, ContactState::Active, 12.0, 10.0)],
            ))
            .unwrap();
        assert!(!moved
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::ButtonDown(MouseButton::Left))));
        assert!(moved
            .events
            .iter()
            .any(|event| matches!(event, OutputEvent::PointerMove { .. })));
        assert!(!arbiter.is_synthetic_left_held());
        assert!(!arbiter.is_left_held());
    }
}
