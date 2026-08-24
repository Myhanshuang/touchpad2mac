use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactState, ContinuousGestureKind, ContinuousGesturePhase,
    M14Profile, Millimeters, Monotonic, OutputEvent, PhysicalButtons,
};

fn c(id: i32, slot: u32, state: ContactState, x: f32, y: f32, major: Option<f32>) -> Contact {
    let mut c = Contact::new(id, slot, state);
    c.x_mm = Some(Millimeters::try_new(x).unwrap());
    c.y_mm = Some(Millimeters::try_new(y).unwrap());
    c.major_mm = major.map(|v| Millimeters::try_new(v).unwrap());
    c
}

fn frame(seq: u64, ms: u64, contacts: Vec<Contact>) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts,
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn gesture(
    decision: &touchpad_core::FrameDecision,
) -> Option<touchpad_core::ContinuousGestureEvent> {
    decision.events.iter().find_map(|event| match event {
        OutputEvent::ContinuousGesture(gesture) => Some(*gesture),
        _ => None,
    })
}

#[test]
fn pinch_wins_before_scroll_and_has_begin_update_end() {
    let mut a = Arbiter::new(M14Profile::new().unwrap().arbiter_config());
    a.frame(&frame(
        1,
        0,
        vec![
            c(1, 0, ContactState::Began, 10.0, 10.0, None),
            c(2, 1, ContactState::Began, 20.0, 10.0, None),
        ],
    ))
    .unwrap();
    let begin = a
        .frame(&frame(
            2,
            10,
            vec![
                c(1, 0, ContactState::Active, 9.0, 10.0, None),
                c(2, 1, ContactState::Active, 21.0, 10.0, None),
            ],
        ))
        .unwrap();
    let g = gesture(&begin).unwrap();
    assert_eq!(g.kind, ContinuousGestureKind::Pinch);
    assert_eq!(g.phase, ContinuousGesturePhase::Begin);
    assert!(!begin.events.iter().any(|e| matches!(
        e,
        OutputEvent::ScrollBegin | OutputEvent::ScrollDelta { .. }
    )));

    let update = a
        .frame(&frame(
            3,
            20,
            vec![
                c(1, 0, ContactState::Active, 8.0, 10.0, None),
                c(2, 1, ContactState::Active, 22.0, 10.0, None),
            ],
        ))
        .unwrap();
    assert_eq!(
        gesture(&update).unwrap().phase,
        ContinuousGesturePhase::Update
    );
    let end = a.frame(&frame(4, 30, vec![])).unwrap();
    assert_eq!(gesture(&end).unwrap().phase, ContinuousGesturePhase::End);
}

#[test]
fn ordinary_two_finger_translation_still_commits_m12_scroll() {
    let mut a = Arbiter::new(M14Profile::new().unwrap().arbiter_config());
    a.frame(&frame(
        1,
        0,
        vec![
            c(1, 0, ContactState::Began, 10.0, 10.0, None),
            c(2, 1, ContactState::Began, 20.0, 10.0, None),
        ],
    ))
    .unwrap();
    let d = a
        .frame(&frame(
            2,
            10,
            vec![
                c(1, 0, ContactState::Active, 10.0, 12.0, None),
                c(2, 1, ContactState::Active, 20.0, 12.0, None),
            ],
        ))
        .unwrap();
    assert!(d
        .events
        .iter()
        .any(|e| matches!(e, OutputEvent::ScrollBegin)));
    assert!(gesture(&d).is_none());
}

#[test]
fn three_finger_swipe_owns_contacts_once_committed() {
    let mut a = Arbiter::new(M14Profile::new().unwrap().arbiter_config());
    a.frame(&frame(
        1,
        0,
        vec![
            c(1, 0, ContactState::Began, 10.0, 10.0, None),
            c(2, 1, ContactState::Began, 20.0, 10.0, None),
            c(3, 2, ContactState::Began, 30.0, 10.0, None),
        ],
    ))
    .unwrap();
    let d = a
        .frame(&frame(
            2,
            10,
            vec![
                c(1, 0, ContactState::Active, 13.0, 10.0, None),
                c(2, 1, ContactState::Active, 23.0, 10.0, None),
                c(3, 2, ContactState::Active, 33.0, 10.0, None),
            ],
        ))
        .unwrap();
    assert_eq!(
        gesture(&d).unwrap().kind,
        ContinuousGestureKind::ThreeFingerSwipe
    );
    assert!(!d.events.iter().any(|e| matches!(
        e,
        OutputEvent::PointerMove { .. } | OutputEvent::ScrollBegin
    )));
}

#[test]
fn thumb_three_uses_m13_metadata_not_geometry_guessing() {
    let mut a = Arbiter::new(M14Profile::new().unwrap().arbiter_config());
    a.frame(&frame(
        1,
        0,
        vec![
            c(1, 0, ContactState::Began, 0.0, 10.0, Some(9.0)),
            c(2, 1, ContactState::Began, 4.0, 10.0, None),
            c(3, 2, ContactState::Began, 8.0, 10.0, None),
            c(4, 3, ContactState::Began, 12.0, 10.0, None),
        ],
    ))
    .unwrap();
    let d = a
        .frame(&frame(
            2,
            10,
            vec![
                c(1, 0, ContactState::Active, 2.0, 10.0, Some(9.0)),
                c(2, 1, ContactState::Active, 5.0, 10.0, None),
                c(3, 2, ContactState::Active, 7.0, 10.0, None),
                c(4, 3, ContactState::Active, 10.0, 10.0, None),
            ],
        ))
        .unwrap();
    assert_eq!(
        gesture(&d).unwrap().kind,
        ContinuousGestureKind::ThumbThreePinch
    );
}
