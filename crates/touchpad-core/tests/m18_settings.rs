use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactState, DesktopAction, GestureTarget, GestureTrigger,
    M18Profile, Millimeters, Monotonic, OutputEvent, PhysicalButtons, UserSettings,
};

fn contact(id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
    let mut contact = Contact::new(id, slot, state);
    contact.x_mm = Some(Millimeters::try_new(x).unwrap());
    contact.y_mm = Some(Millimeters::try_new(y).unwrap());
    contact
}

fn four(seq: u64, ms: u64, state: ContactState, y: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![
            contact(1, 0, state, 30.0, y),
            contact(2, 1, state, 40.0, y),
            contact(3, 2, state, 50.0, y),
            contact(4, 3, state, 60.0, y),
        ],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn three(seq: u64, ms: u64, state: ContactState, y: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![
            contact(1, 0, state, 30.0, y),
            contact(2, 1, state, 40.0, y),
            contact(3, 2, state, 50.0, y),
        ],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

#[test]
fn mapped_four_finger_swipe_emits_one_desktop_action() {
    let mut settings = UserSettings::default();
    settings
        .gestures
        .set_target(
            GestureTrigger::FourFingerSwipeUp,
            GestureTarget::ShowDesktop,
        )
        .unwrap();
    let config = M18Profile::new(settings).unwrap().arbiter_config().unwrap();
    let mut arbiter = Arbiter::new(config);

    arbiter
        .frame(&four(1, 0, ContactState::Began, 30.0))
        .unwrap();
    let committed = arbiter
        .frame(&four(2, 10, ContactState::Active, 27.0))
        .unwrap();
    assert_eq!(
        committed
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    OutputEvent::DesktopAction(DesktopAction::ShowDesktop)
                )
            })
            .count(),
        1
    );
    assert!(!committed
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::ContinuousGesture(_))));

    let update = arbiter
        .frame(&four(3, 20, ContactState::Active, 24.0))
        .unwrap();
    assert!(!update
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::DesktopAction(_))));
    assert!(!update
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::ContinuousGesture(_))));
}

#[test]
fn four_finger_swipe_down_can_request_explicit_overview_close() {
    let mut settings = UserSettings::default();
    settings
        .gestures
        .set_target(
            GestureTrigger::FourFingerSwipeDown,
            GestureTarget::CloseOverview,
        )
        .unwrap();
    let config = M18Profile::new(settings).unwrap().arbiter_config().unwrap();
    let mut arbiter = Arbiter::new(config);

    arbiter
        .frame(&four(1, 0, ContactState::Began, 30.0))
        .unwrap();
    let committed = arbiter
        .frame(&four(2, 10, ContactState::Active, 33.0))
        .unwrap();
    assert_eq!(
        committed.events,
        vec![OutputEvent::DesktopAction(DesktopAction::CloseOverview)]
    );
}

#[test]
fn macos_preset_disables_drag_commit_so_three_finger_swipe_is_reachable() {
    let settings = UserSettings::macos_inspired();
    assert!(!settings.gestures.three_finger_drag_enabled);
    let config = M18Profile::new(settings).unwrap().arbiter_config().unwrap();
    assert!(!config
        .three_finger_drag_config()
        .expect("M18 retains three-finger tap candidate stage")
        .drag_enabled());
    let mut arbiter = Arbiter::new(config);

    arbiter
        .frame(&three(1, 0, ContactState::Began, 30.0))
        .unwrap();
    let committed = arbiter
        .frame(&three(2, 10, ContactState::Active, 27.0))
        .unwrap();
    assert!(committed
        .events
        .contains(&OutputEvent::DesktopAction(DesktopAction::OpenOverview)));
    assert!(!committed.events.iter().any(|event| {
        matches!(
            event,
            OutputEvent::ButtonDown(touchpad_core::MouseButton::Left)
        )
    }));
}

#[test]
fn disabling_three_finger_drag_commit_keeps_three_finger_tap_mapping() {
    let mut settings = UserSettings::macos_inspired();
    settings
        .set_key("gesture.three-finger-tap", "lookup")
        .unwrap();
    let config = M18Profile::new(settings).unwrap().arbiter_config().unwrap();
    let mut arbiter = Arbiter::new(config);

    arbiter
        .frame(&three(1, 0, ContactState::Began, 30.0))
        .unwrap();
    let released = arbiter
        .frame(&ContactFrame {
            monotonic_timestamp: Monotonic::from_nanos(80_000_000),
            sequence: 2,
            discontinuity: false,
            contacts: vec![],
            physical_buttons: PhysicalButtons::NONE,
            diagnostics: vec![],
        })
        .unwrap();
    assert!(released
        .events
        .contains(&OutputEvent::DesktopAction(DesktopAction::Lookup)));
}
