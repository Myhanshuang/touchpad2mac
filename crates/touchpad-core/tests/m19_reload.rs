use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactState, M19Profile, Millimeters, Monotonic,
    PhysicalButtons, UserSettings,
};

fn one(seq: u64, ms: u64, state: ContactState, x: f32) -> ContactFrame {
    let mut contact = Contact::new(1, 0, state);
    contact.x_mm = Some(Millimeters::try_new(x).unwrap());
    contact.y_mm = Some(Millimeters::try_new(20.0).unwrap());
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![contact],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn empty(seq: u64, ms: u64) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence: seq,
        discontinuity: false,
        contacts: vec![],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

#[test]
fn config_replacement_waits_for_neutral_boundary() {
    let initial = M19Profile::new(UserSettings::default())
        .unwrap()
        .arbiter_config()
        .unwrap();
    let mut arbiter = Arbiter::new(initial);
    arbiter
        .frame(&one(1, 0, ContactState::Began, 10.0))
        .unwrap();
    assert!(!arbiter.is_settings_quiescent());

    let mut tuned = UserSettings::default();
    tuned.set_key("feel.pointer.tracking_speed", "1.5").unwrap();
    let replacement = M19Profile::new(tuned).unwrap().arbiter_config().unwrap();
    assert!(!arbiter.try_replace_config(replacement.clone()));

    arbiter.frame(&empty(2, 30)).unwrap();
    assert!(arbiter.is_settings_quiescent());
    assert!(arbiter.try_replace_config(replacement));
    assert_eq!(
        arbiter.config().fidelity_config().unwrap().tracking_speed(),
        1.5
    );
}
