use std::time::Duration;

use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactState, DesktopAction, M15Profile, Millimeters,
    Monotonic, MouseButton, OutputEvent, PhysicalButtons, ThreeFingerDragConfig,
};

fn c(id: i32, slot: u32, state: ContactState, x: f32) -> Contact {
    let mut c = Contact::new(id, slot, state);
    c.x_mm = Some(Millimeters::try_new(x).unwrap());
    c.y_mm = Some(Millimeters::try_new(10.0).unwrap());
    c
}

fn three(seq: u64, ms: u64, state: ContactState, x: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![
            c(1, 0, state, x),
            c(2, 1, state, x + 5.0),
            c(3, 2, state, x + 10.0),
        ],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn two_remaining(seq: u64, ms: u64, x: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![
            c(1, 0, ContactState::Active, x),
            c(2, 1, ContactState::Active, x + 5.0),
        ],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn one_remaining(seq: u64, ms: u64, x: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![c(1, 0, ContactState::Active, x)],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

#[test]
fn drag_commits_before_swipe_and_reuses_pointer_fidelity() {
    let mut a = Arbiter::new(M15Profile::new().unwrap().arbiter_config());
    a.frame(&three(1, 0, ContactState::Began, 0.0)).unwrap();
    let d = a.frame(&three(2, 10, ContactState::Active, 1.2)).unwrap();
    assert!(matches!(
        d.events.first(),
        Some(OutputEvent::ButtonDown(MouseButton::Left))
    ));
    assert!(d
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::PointerMove { .. })));
    assert!(!d
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::ContinuousGesture(_))));
}

#[test]
fn staggered_lift_keeps_two_finger_tail_owned_and_releases_at_one_finger() {
    let config = M15Profile::new()
        .unwrap()
        .arbiter_config()
        .with_three_finger_drag(
            ThreeFingerDragConfig::new(1.0, 0.5, Duration::from_millis(200), false).unwrap(),
        );
    let mut a = Arbiter::new(config);

    a.frame(&three(1, 0, ContactState::Began, 0.0)).unwrap();
    let begin = a.frame(&three(2, 10, ContactState::Active, 1.2)).unwrap();
    assert!(begin
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::ButtonDown(MouseButton::Left))));

    // One lifted finger (3 -> 2) keeps ownership: no early up and no
    // lower-policy pointer/scroll leakage.
    let partial = a.frame(&two_remaining(3, 16, 1.3)).unwrap();
    assert!(partial.events.is_empty(), "{:?}", partial.events);
    assert!(a.is_left_held());

    // Once only one original finger remains (3 -> 1), match libinput's
    // release boundary and end the drag immediately on that frame.
    let ended = a.frame(&one_remaining(4, 22, 1.3)).unwrap();
    assert_eq!(
        ended
            .events
            .iter()
            .filter(|event| matches!(event, OutputEvent::ButtonUp(MouseButton::Left)))
            .count(),
        1
    );
    assert!(!a.is_left_held());
}

#[test]
fn clean_lift_locks_drag_and_next_three_finger_tap_releases() {
    let mut a = Arbiter::new(M15Profile::new().unwrap().arbiter_config());
    a.frame(&three(1, 0, ContactState::Began, 0.0)).unwrap();
    a.frame(&three(2, 10, ContactState::Active, 1.2)).unwrap();
    let lift = a.frame(&frame_empty(3, 20)).unwrap();
    assert!(!lift
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::ButtonUp(MouseButton::Left))));

    a.frame(&three(4, 30, ContactState::Began, 2.0)).unwrap();
    let release = a.frame(&frame_empty(5, 80)).unwrap();
    assert_eq!(
        release
            .events
            .iter()
            .filter(|event| matches!(event, OutputEvent::ButtonUp(MouseButton::Left)))
            .count(),
        1
    );
}

#[test]
fn short_three_finger_tap_is_lookup_semantic_action() {
    let mut a = Arbiter::new(M15Profile::new().unwrap().arbiter_config());
    a.frame(&three(1, 0, ContactState::Began, 0.0)).unwrap();
    let release = a.frame(&frame_empty(2, 100)).unwrap();
    assert!(release
        .events
        .contains(&OutputEvent::DesktopAction(DesktopAction::Lookup)));
}

fn frame_empty(seq: u64, ms: u64) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}
