//! Public integration tests for M13 contact robustness.

use std::time::Duration;

use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactRole, ContactState, M12Profile, Millimeters, Monotonic,
    OutputEvent, PhysicalButtons, RobustnessConfig,
};

fn contact(id: i32, state: ContactState, x: f32, y: f32, major: Option<f32>) -> Contact {
    let mut c = Contact::new(id, id as u32, state);
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

fn config(surface: bool) -> touchpad_core::ArbiterConfig {
    let robust = RobustnessConfig::new(12.0, 8.0, 3.0, 0.06, Duration::from_millis(500)).unwrap();
    let robust = if surface {
        robust.with_surface_size_mm(131.0, 77.0).unwrap()
    } else {
        robust
    };
    M12Profile::new()
        .unwrap()
        .arbiter_config()
        .with_robustness(robust)
}

#[test]
fn palm_never_enters_pointer_ownership_but_thumb_is_retained() {
    let mut a = Arbiter::new(config(false));
    let palm = a
        .frame(&frame(
            1,
            0,
            vec![contact(1, ContactState::Began, 20.0, 20.0, Some(13.0))],
        ))
        .unwrap();
    assert!(palm.events.is_empty());
    assert_eq!(a.tracking_id(), None);
    assert_eq!(a.contact_role(1), Some(ContactRole::Palm));

    let thumb = a
        .frame(&frame(
            2,
            10,
            vec![
                contact(1, ContactState::Active, 20.0, 20.0, Some(13.0)),
                contact(2, ContactState::Began, 30.0, 20.0, Some(9.0)),
            ],
        ))
        .unwrap();
    assert!(thumb.events.is_empty());
    assert_eq!(a.contact_role(2), Some(ContactRole::Thumb));
    assert_eq!(a.tracking_id(), Some(2));
}

#[test]
fn typing_signal_suppresses_only_new_contacts_inside_window() {
    let mut a = Arbiter::new(config(false));
    a.note_typing(Monotonic::ZERO);
    a.frame(&frame(
        1,
        100,
        vec![contact(1, ContactState::Began, 20.0, 20.0, None)],
    ))
    .unwrap();
    assert_eq!(a.contact_role(1), Some(ContactRole::TypingSuppressed));
    assert_eq!(a.tracking_id(), None);
    a.frame(&frame(
        2,
        700,
        vec![contact(2, ContactState::Began, 20.0, 20.0, None)],
    ))
    .unwrap();
    assert_eq!(a.contact_role(2), Some(ContactRole::Finger));
    assert_eq!(a.tracking_id(), Some(2));
}

#[test]
fn edge_start_stays_suppressed_after_moving_to_center() {
    let mut a = Arbiter::new(config(true));
    a.frame(&frame(
        1,
        0,
        vec![contact(1, ContactState::Began, 1.0, 30.0, None)],
    ))
    .unwrap();
    a.frame(&frame(
        2,
        10,
        vec![contact(1, ContactState::Active, 60.0, 30.0, None)],
    ))
    .unwrap();
    assert_eq!(a.contact_role(1), Some(ContactRole::EdgeSuppressed));
    assert_eq!(a.tracking_id(), None);
}

#[test]
fn jitter_hold_prevents_sub_radius_pointer_commit() {
    let mut a = Arbiter::new(config(false));
    a.frame(&frame(
        1,
        0,
        vec![contact(1, ContactState::Began, 20.0, 20.0, None)],
    ))
    .unwrap();
    let tiny = a
        .frame(&frame(
            2,
            10,
            vec![contact(1, ContactState::Active, 20.03, 20.0, None)],
        ))
        .unwrap();
    assert!(tiny.events.is_empty());
    let moved = a
        .frame(&frame(
            3,
            20,
            vec![contact(1, ContactState::Active, 21.2, 20.0, None)],
        ))
        .unwrap();
    assert!(moved
        .events
        .iter()
        .any(|event| matches!(event, OutputEvent::PointerMove { .. })));
}

#[test]
fn m12_profile_has_no_robustness_stage() {
    assert!(!M12Profile::new()
        .unwrap()
        .arbiter_config()
        .is_robustness_enabled());
}
