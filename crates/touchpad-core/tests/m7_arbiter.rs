//! Public-contract tests for the M7 Interaction Arbiter.
//!
//! These tests use only the crate's public API (mirroring how `touchpad-linux`
//! and future milestone crates consume the arbiter): configuration
//! validation, the observable lifecycle, the idempotent release path, and the
//! `ArbiterSink` adapter with a fault-injecting fake sink. No real output
//! sink is ever instantiated.

use touchpad_core::{
    Arbiter, ArbiterConfig, ArbiterConfigError, ArbiterError, ArbiterSink, ArbiterSinkError,
    Contact, ContactFrame, ContactState, DiagnosticCode, DiagnosticLevel, Lifecycle,
    LifecycleTransition, LogicalPixels, LogicalPixelsPerMm, Millimeters, Monotonic, MouseButton,
    OutputError, OutputEvent, OutputSink, PhysicalButtons,
};

fn mm(x: f32) -> Millimeters {
    Millimeters::try_new(x).unwrap()
}

fn px(x: f32) -> LogicalPixels {
    LogicalPixels::try_new(x).unwrap()
}

fn cfg() -> ArbiterConfig {
    ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(10.0).unwrap()).unwrap()
}

fn contact(tracking_id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
    let mut c = Contact::new(tracking_id, slot, state);
    c.x_mm = Some(mm(x));
    c.y_mm = Some(mm(y));
    c
}

fn frame(
    sequence: u64,
    ts: u64,
    contacts: Vec<Contact>,
    left: bool,
    discontinuity: bool,
) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ts),
        sequence,
        discontinuity,
        contacts,
        physical_buttons: PhysicalButtons::new(left, false, false),
        diagnostics: vec![],
    }
}

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: px(dx),
        dy: px(dy),
    }
}

fn down() -> OutputEvent {
    OutputEvent::ButtonDown(MouseButton::Left)
}

fn up() -> OutputEvent {
    OutputEvent::ButtonUp(MouseButton::Left)
}

#[test]
fn config_validation_is_public_and_strict() {
    // Threshold must be strictly positive.
    assert_eq!(
        ArbiterConfig::new(mm(0.0), LogicalPixelsPerMm::try_new(10.0).unwrap()),
        Err(ArbiterConfigError::NonPositiveThreshold(mm(0.0)))
    );
    assert_eq!(
        ArbiterConfig::new(mm(-2.0), LogicalPixelsPerMm::try_new(10.0).unwrap()),
        Err(ArbiterConfigError::NonPositiveThreshold(mm(-2.0)))
    );
    // Scale must be finite and strictly positive.
    assert!(LogicalPixelsPerMm::try_new(f32::NAN).is_err());
    assert!(LogicalPixelsPerMm::try_new(0.0).is_err());
    assert!(LogicalPixelsPerMm::try_new(-1.0).is_err());
    // A valid configuration exposes its values.
    let config = cfg();
    assert_eq!(config.motion_threshold_mm(), mm(1.0));
    assert_eq!(config.logical_pixels_per_mm().as_px_per_mm(), 10.0);
}

#[test]
fn public_lifecycle_observability_through_a_full_interaction() {
    let mut arbiter = Arbiter::new(cfg());
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(arbiter.tracking_id(), None);

    let d = arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Candidate);
    assert_eq!(
        d.transitions,
        vec![LifecycleTransition::Begin { tracking_id: 1 }]
    );
    assert!(d.events.is_empty());
    assert_eq!(arbiter.tracking_id(), Some(1));

    let d = arbiter
        .frame(&frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Committed);
    assert_eq!(
        d.transitions,
        vec![LifecycleTransition::Commit { tracking_id: 1 }]
    );
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);

    let d = arbiter
        .frame(&frame(
            2,
            2,
            vec![contact(1, 0, ContactState::Ended, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Finished);
    assert_eq!(
        d.transitions,
        vec![LifecycleTransition::Finish { tracking_id: 1 }]
    );
    assert!(d.events.is_empty());
}

#[test]
fn regression_and_invalid_frame_errors_are_public_and_structured() {
    let mut arbiter = Arbiter::new(cfg());
    arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();

    // Duplicate slot: structured invalid-frame error, state untouched.
    let mut bad = frame(
        1,
        1,
        vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
        false,
        false,
    );
    bad.contacts
        .push(contact(1, 0, ContactState::Active, 3.0, 0.0));
    match arbiter.frame(&bad) {
        Err(ArbiterError::InvalidFrame { sequence, .. }) => assert_eq!(sequence, 1),
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);

    // Sequence regression: structured error and deterministic cancel.
    match arbiter.frame(&frame(0, 2, vec![], false, false)) {
        Err(ArbiterError::SequenceRegression { found, previous }) => {
            assert_eq!(found, 0);
            assert_eq!(previous, 0);
        }
        other => panic!("expected SequenceRegression, got {other:?}"),
    }
    assert_eq!(arbiter.lifecycle(), Lifecycle::Cancelled);
}

#[test]
fn release_all_is_public_idempotent_and_resets() {
    let mut arbiter = Arbiter::new(cfg());
    arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    assert!(arbiter.is_left_held());

    let events = arbiter.release_all();
    assert_eq!(events, vec![OutputEvent::ButtonUp(MouseButton::Left)]);
    assert!(!arbiter.is_left_held());
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(arbiter.tracking_id(), None);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // Idempotent: a second release emits nothing.
    assert_eq!(arbiter.release_all(), Vec::<OutputEvent>::new());
}

/// A scripted fault-injecting sink with a **real held-state model**: rejects
/// specific submission indices, records accepted events, and can fail its own
/// cleanup a configured number of times before succeeding.
///
/// The held-state model mirrors the `OutputSink` contract: an accepted
/// `ButtonDown(Left)` sets held, an accepted `ButtonUp(Left)` clears held,
/// and a *successful* wrapped `release_all()` clears held (it releases all
/// held button/key state). A rejected submit never changes held state. This
/// lets the cleanup tests assert exactly what the sink holds after each
/// attempt.
struct ScriptedSink {
    events: Vec<OutputEvent>,
    reject_submits: Vec<usize>,
    submits: usize,
    release_failures_left: usize,
    releases: usize,
    /// Whether this sink itself currently holds the left button.
    held_left: bool,
}

impl ScriptedSink {
    fn new(reject_submits: Vec<usize>) -> Self {
        Self {
            events: Vec::new(),
            reject_submits,
            submits: 0,
            release_failures_left: 0,
            releases: 0,
            held_left: false,
        }
    }

    fn with_release_failures(mut self, failures: usize) -> Self {
        self.release_failures_left = failures;
        self
    }
}

impl OutputSink for ScriptedSink {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        let index = self.submits;
        self.submits += 1;
        if self.reject_submits.contains(&index) {
            return Err(OutputError::Rejected(event));
        }
        // A rejected submit never changes held state; an accepted event
        // updates the real held-state model.
        match &event {
            OutputEvent::ButtonDown(MouseButton::Left) => self.held_left = true,
            OutputEvent::ButtonUp(MouseButton::Left) => self.held_left = false,
            _ => {}
        }
        self.events.push(event);
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        self.releases += 1;
        if self.release_failures_left > 0 {
            self.release_failures_left -= 1;
            return Err(OutputError::Io("scripted sink release_all failure".into()));
        }
        // A successful wrapped cleanup releases all held state.
        self.held_left = false;
        Ok(())
    }
}

#[test]
fn arbiter_sink_delivers_events_in_decision_order() {
    // Happy path: events flow to the sink in decision order.
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![usize::MAX]));
    adapter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    adapter
        .frame(&frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    assert_eq!(
        adapter.sink().events,
        vec![
            OutputEvent::ButtonDown(MouseButton::Left),
            move_event(20.0, 0.0),
        ]
    );
}

/// A rejected `ButtonDown` is not treated as delivered: the arbiter does not
/// track it as held and cleanup emits no unmatched up.
#[test]
fn arbiter_sink_rejected_down_is_not_held_and_releases_nothing() {
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![0]));
    let err = adapter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap_err();
    match err {
        ArbiterSinkError::PartialSubmit {
            index,
            accepted_prefix,
            decision_len,
            failed_event,
            primary,
        } => {
            assert_eq!(index, 0);
            assert_eq!(accepted_prefix, 0);
            assert_eq!(decision_len, 1);
            assert_eq!(failed_event, OutputEvent::ButtonDown(MouseButton::Left));
            assert!(matches!(primary, OutputError::Rejected(_)));
        }
        other => panic!("expected PartialSubmit, got {other:?}"),
    }
    // The rejected down is NOT tracked as held (no unmatched up possible).
    assert!(!adapter.arbiter().is_left_held());
    assert!(adapter.is_faulted());
    // Normal frames are blocked while faulted.
    assert!(matches!(
        adapter.frame(&frame(1, 1, vec![], true, false)),
        Err(ArbiterSinkError::Faulted)
    ));
    // Cleanup submits nothing and resets the adapter.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, Vec::<OutputEvent>::new()); // no unmatched up
}

/// An accepted down followed by a failed movement stays delivered-held; the
/// cleanup path releases it exactly once and the adapter recovers for a fresh
/// interaction.
#[test]
fn arbiter_sink_failed_movement_after_accepted_down_releases_once_and_recovers() {
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![2]));
    // Begin + commit: the move (20,0) is submission 0.
    adapter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    adapter
        .frame(&frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Press + drag: decision [down, move 5,0]; the down (submission 1) is
    // accepted and the movement (submission 2) is rejected.
    let err = adapter
        .frame(&frame(
            2,
            2,
            vec![contact(1, 0, ContactState::Active, 2.5, 0.0)],
            true,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::PartialSubmit {
            index: 1,
            accepted_prefix: 1,
            decision_len: 2,
            ..
        }
    ));
    assert!(adapter.arbiter().is_left_held()); // the accepted down stays held
    assert!(adapter.sink().held_left);
    assert!(adapter.is_faulted());
    // Cleanup delivers the matching up exactly once, then resets.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![move_event(20.0, 0.0), down(), up()]);
    assert!(!sink.held_left);
}

/// R3 quadrant 2 (public API): the explicit up submission *and* the wrapped
/// sink's cleanup both fail, so the release stays owed; the next cleanup
/// retries the explicit up and a fresh interaction works after recovery.
#[test]
fn arbiter_sink_failed_release_is_retried_and_fresh_interaction_works() {
    // Submission 0 (down) accepted; submission 1 (the first cleanup up) is
    // rejected; the first wrapped release_all also fails — neither
    // acknowledgement succeeds.
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![1]).with_release_failures(1));
    adapter.frame(&frame(0, 0, vec![], true, false)).unwrap();
    let err = adapter.release_all().unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::ReleaseFailed {
            primary: Some(OutputError::Rejected(_)),
            cleanup: Some(_),
            ..
        }
    ));
    // A failed cleanup does NOT erase the owed release: both the sink and the
    // adapter still hold, and the arbiter is not reset.
    assert!(adapter.arbiter().is_left_held());
    assert!(adapter.sink().held_left);
    // The next cleanup retries the explicit up; it is accepted, the wrapped
    // cleanup now succeeds, and the adapter resets.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![down(), up()]);
    assert_eq!(sink.submits, 3); // down + rejected up + retried up
    assert_eq!(sink.releases, 2);
    assert!(!sink.held_left);

    // A fresh interaction after recovery produces exactly one down/up.
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![usize::MAX]));
    adapter
        .frame(&frame(
            0,
            0,
            vec![contact(9, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    adapter
        .frame(&frame(
            1,
            1,
            vec![contact(9, 0, ContactState::Active, 2.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    adapter
        .frame(&frame(
            2,
            2,
            vec![contact(9, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    adapter.release_all().unwrap();
    let (_, sink) = adapter.into_parts();
    assert_eq!(sink.events, vec![down(), move_event(20.0, 0.0), up()]);
}

/// R3 quadrant 1 (public API): the explicit up submission fails but the
/// wrapped sink's own cleanup succeeds. The wrapped cleanup is authoritative
/// — a successful `release_all()` released all held state — so the release is
/// reconciled to delivered and the explicit failure is reported; the recovery
/// call must NOT submit another (duplicate/unmatched) up.
#[test]
fn arbiter_sink_wrapped_cleanup_is_authoritative_after_failed_explicit_up() {
    // Submission 0 (down) accepted; submission 1 (the first cleanup up) is
    // rejected; the wrapped release_all succeeds immediately.
    let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![1]));
    adapter.frame(&frame(0, 0, vec![], true, false)).unwrap();
    assert!(adapter.sink().held_left);
    let err = adapter.release_all().unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::ReleaseFailed {
            primary: Some(OutputError::Rejected(_)),
            cleanup: None,
            ..
        }
    ));
    // The wrapped cleanup succeeded, so the sink holds nothing and the
    // adapter's delivery knowledge is reconciled to released.
    assert!(!adapter.sink().held_left);
    assert!(!adapter.arbiter().is_left_held());
    assert!(!adapter.is_faulted());
    // Recovery: no second up is attempted (the wrapped cleanup already
    // released everything); the adapter just re-acknowledges and resets.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![down()]); // the explicit up was rejected
    assert_eq!(sink.submits, 2); // down + rejected up; no second up attempt
    assert_eq!(sink.releases, 2);
    assert!(!sink.held_left);
}

/// The wrapped sink's own cleanup contract is invoked, and its failure is
/// preserved and retried.
#[test]
fn arbiter_sink_wrapped_release_all_failure_is_retried() {
    let mut adapter = ArbiterSink::new(
        cfg(),
        ScriptedSink::new(vec![usize::MAX]).with_release_failures(1),
    );
    adapter.frame(&frame(0, 0, vec![], true, false)).unwrap();
    let err = adapter.release_all().unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::ReleaseFailed {
            primary: None,
            cleanup: Some(_),
            ..
        }
    ));
    // The explicit up was delivered (the sink no longer holds); the wrapped
    // cleanup is retried next time — never a second up.
    assert!(!adapter.sink().held_left);
    assert!(!adapter.arbiter().is_left_held());
    assert!(!adapter.is_faulted());
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![down(), up()]);
    assert_eq!(sink.submits, 2); // down + up; no second up attempt
    assert_eq!(sink.releases, 2);
    assert!(!sink.held_left);
}

// --------------------------------------------------------------------------
// R2: model validation (ContactFrame::validate) rejection
// --------------------------------------------------------------------------

/// A live contact with a negative tracking id is an Error diagnostic from the
/// core model; the arbiter rejects the frame atomically via
/// `ContactFrame::validate`.
#[test]
fn negative_live_tracking_id_frame_is_rejected_atomically() {
    let mut arbiter = Arbiter::new(cfg());
    arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let mut bad = contact(1, 0, ContactState::Active, 2.0, 0.0);
    bad.tracking_id = -5;
    match arbiter.frame(&frame(1, 1, vec![bad], false, false)) {
        Err(ArbiterError::InvalidFrame {
            sequence,
            codes,
            reason,
        }) => {
            assert_eq!(sequence, 1);
            assert!(codes.contains(&DiagnosticCode::InvalidEventOrder));
            assert!(!reason.is_empty());
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
    // State unchanged: still a candidate at the old anchor.
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);
    assert_eq!(arbiter.tracking_id(), Some(1));
    // A subsequent valid frame continues normally.
    let d = arbiter
        .frame(&frame(
            2,
            2,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Committed);
}

/// Applies one of the invalid-field mutations used by
/// [`invalid_pressure_orientation_ellipse_frames_are_rejected`].
fn apply_invalid_field(c: &mut Contact, case: usize) {
    match case {
        0 => c.pressure = Some(1.5),
        1 => c.pressure = Some(f32::NAN),
        2 => c.orientation = Some(f32::INFINITY),
        3 => c.major_mm = Some(mm(-1.0)),
        _ => c.minor_mm = Some(mm(-0.25)),
    }
}

/// Invalid pressure, orientation, and ellipse axes are Error diagnostics;
/// each rejects the frame wholesale with the structured code.
#[test]
fn invalid_pressure_orientation_ellipse_frames_are_rejected() {
    let cases: &[(usize, DiagnosticCode)] = &[
        (0, DiagnosticCode::OutOfRangeValue), // pressure > 1
        (1, DiagnosticCode::NonFiniteValue),  // pressure NaN
        (2, DiagnosticCode::NonFiniteValue),  // orientation infinite
        (3, DiagnosticCode::OutOfRangeValue), // negative major axis
        (4, DiagnosticCode::OutOfRangeValue), // negative minor axis
    ];
    for &(case, expected_code) in cases {
        let mut c = contact(7, 0, ContactState::Began, 0.0, 0.0);
        apply_invalid_field(&mut c, case);
        match Arbiter::new(cfg()).frame(&frame(0, 0, vec![c], false, false)) {
            Err(ArbiterError::InvalidFrame { codes, .. }) => {
                assert!(
                    codes.contains(&expected_code),
                    "case {case}: expected {expected_code:?}, got {codes:?}"
                );
            }
            other => panic!("case {case}: expected InvalidFrame, got {other:?}"),
        }
    }
}

/// An incomplete `Began` contact is Warning-only: the frame is accepted, no
/// candidate is created, and the arbiter emits its own warning diagnostic
/// (the M7 warning-only policy is preserved).
#[test]
fn incomplete_began_contact_is_warning_only() {
    let mut arbiter = Arbiter::new(cfg());
    let incomplete = Contact::new(1, 0, ContactState::Began); // no coordinates
    let d = arbiter
        .frame(&frame(0, 0, vec![incomplete], false, false))
        .expect("warning-only frame is accepted");
    assert_eq!(d.lifecycle_after, Lifecycle::Idle); // no candidate
    assert!(d.events.is_empty());
    assert!(d.diagnostics.iter().any(|d| {
        d.level == DiagnosticLevel::Warning && d.code == DiagnosticCode::IncompleteNewContact
    }));
    // A complete contact afterwards begins a normal candidate.
    let d = arbiter
        .frame(&frame(
            1,
            1,
            vec![contact(2, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Candidate);
}

/// State atomicity when an invalid frame also changes the physical button
/// bit: the frame is rejected and neither the button edge nor any other state
/// is applied.
#[test]
fn invalid_frame_with_button_edge_leaves_state_and_buttons_untouched() {
    let mut arbiter = Arbiter::new(cfg());
    arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Invalid frame (negative live tracking id) that also presses left.
    let mut bad = contact(1, 0, ContactState::Active, 2.0, 0.0);
    bad.tracking_id = -1;
    match arbiter.frame(&frame(1, 1, vec![bad], true, false)) {
        Err(ArbiterError::InvalidFrame { codes, .. }) => {
            assert!(codes.contains(&DiagnosticCode::InvalidEventOrder));
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
    // Nothing was applied: no button held, lifecycle unchanged, baseline not
    // advanced.
    assert!(!arbiter.is_left_held());
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);
    // The next valid frame (same sequence would still be accepted) commits
    // and delivers the press edge.
    let d = arbiter
        .frame(&frame(
            2,
            2,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![down(), move_event(20.0, 0.0)]);
    assert!(arbiter.is_left_held());
}

#[test]
fn decisions_are_serializable() {
    let mut arbiter = Arbiter::new(cfg());
    let d = arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    let json = serde_json::to_string(&d).unwrap();
    let decoded: touchpad_core::FrameDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, d);
}
