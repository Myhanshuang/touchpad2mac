//! Public integration tests for M12 scroll fidelity after moving kinetic
//! continuation out of touchpad-core.

use touchpad_core::{
    Arbiter, Contact, ContactFrame, ContactState, M12Profile, Millimeters, Monotonic, OutputEvent,
    PhysicalButtons,
};

fn contact(id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
    let mut c = Contact::new(id, slot, state);
    c.x_mm = Some(Millimeters::try_new(x).unwrap());
    c.y_mm = Some(Millimeters::try_new(y).unwrap());
    c
}

fn pair_frame(sequence: u64, ms: u64, state: ContactState, x: f32) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence,
        discontinuity: false,
        contacts: vec![
            contact(10, 0, state, x, 0.0),
            contact(11, 1, state, x + 10.0, 0.0),
        ],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn single_began(sequence: u64, ms: u64) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ms * 1_000_000),
        sequence,
        discontinuity: false,
        contacts: vec![contact(20, 0, ContactState::Began, 0.0, 0.0)],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn arbiter() -> Arbiter {
    Arbiter::new(M12Profile::new().unwrap().arbiter_config())
}

fn scroll_delta(decision: &touchpad_core::FrameDecision) -> Option<(f32, f32)> {
    decision.events.iter().find_map(|event| match event {
        OutputEvent::ScrollDelta { dx, dy } => Some((dx.as_px(), dy.as_px())),
        _ => None,
    })
}

fn drive_fast_scroll_to_release(a: &mut Arbiter) -> touchpad_core::FrameDecision {
    a.frame(&pair_frame(1, 0, ContactState::Began, 0.0))
        .unwrap();
    for (seq, ms, x) in [
        (2, 10, 1.0),
        (3, 20, 2.0),
        (4, 30, 3.0),
        (5, 40, 4.0),
        (6, 50, 5.0),
    ] {
        a.frame(&pair_frame(seq, ms, ContactState::Active, x))
            .unwrap();
    }
    a.frame(&pair_frame(7, 60, ContactState::Ended, 5.0))
        .unwrap()
}

#[test]
fn clean_release_ends_finger_scroll_immediately_and_tick_is_inert() {
    let mut a = arbiter();
    let release = drive_fast_scroll_to_release(&mut a);
    assert!(release.events.contains(&OutputEvent::ScrollEnd));
    assert!(!a.is_scroll_momentum_active());
    assert!(!a.is_scroll_open());
    assert_eq!(a.scroll_remainder_px(), (0.0, 0.0));

    let tick = a.tick(Monotonic::from_nanos(76_000_000)).unwrap();
    assert!(tick.events.is_empty());
    assert!(scroll_delta(&tick).is_none());
}

#[test]
fn new_contact_after_release_starts_new_ownership_without_extra_scroll_end() {
    let mut a = arbiter();
    let release = drive_fast_scroll_to_release(&mut a);
    assert_eq!(
        release
            .events
            .iter()
            .filter(|event| matches!(event, OutputEvent::ScrollEnd))
            .count(),
        1
    );
    let d = a.frame(&single_began(8, 70)).unwrap();
    assert_eq!(
        d.events
            .iter()
            .filter(|event| matches!(event, OutputEvent::ScrollEnd))
            .count(),
        0
    );
    assert!(!a.is_scroll_momentum_active());
    assert!(!a.is_scroll_open());
}

#[test]
fn release_all_after_clean_scroll_release_is_empty_and_idempotent() {
    let mut a = arbiter();
    let release = drive_fast_scroll_to_release(&mut a);
    assert!(release.events.contains(&OutputEvent::ScrollEnd));
    assert!(a.release_all().is_empty());
    assert!(!a.is_scroll_momentum_active());
    assert!(!a.is_scroll_open());
    assert!(a.release_all().is_empty());
}

#[test]
fn tick_does_not_advance_input_frame_regression_baseline() {
    let mut a = arbiter();
    let release = drive_fast_scroll_to_release(&mut a);
    assert!(release.events.contains(&OutputEvent::ScrollEnd));
    a.tick(Monotonic::from_nanos(90_000_000)).unwrap();
    // Policy timers use the input-domain clock but do not advance the input
    // frame regression baseline.
    assert!(a.frame(&single_began(8, 70)).is_ok());
}

#[test]
fn m11_profile_keeps_m9_linear_scroll_and_has_no_m12_stage() {
    let m11 = touchpad_core::M11Profile::new().unwrap().arbiter_config();
    assert!(!m11.is_scroll_fidelity_enabled());
    assert!(m11.scroll_fidelity_config().is_none());
}
