//! Public-contract tests for the M8 tap / tap-and-drag / sticky drag lock
//! policy.
//!
//! These tests use only the crate's public API: the validated [`TapConfig`],
//! the observable [`TapDragPhase`] (on `FrameDecision` and `Arbiter`), the
//! aggregate left-button arbitration, and the `ArbiterSink` delivery-aware
//! adapter for synthetic events with a fault-injecting fake sink. No real
//! output sink is ever instantiated.

use std::time::Duration;

use touchpad_core::{
    Arbiter, ArbiterConfig, ArbiterSink, ArbiterSinkError, Contact, ContactFrame, ContactState,
    FrameDecision, Lifecycle, LogicalPixels, LogicalPixelsPerMm, Millimeters, Monotonic,
    MouseButton, OutputError, OutputEvent, OutputSink, PhysicalButtons, TapConfig, TapConfigError,
    TapDragPhase,
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

fn dur(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// Default M8 test tap config: tap + tap-and-drag + sticky drag lock.
fn tap_cfg() -> TapConfig {
    TapConfig::new(true, true, true, dur(500), mm(2.0), dur(400)).unwrap()
}

fn tap_arbiter_cfg() -> ArbiterConfig {
    cfg().with_tap(tap_cfg())
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

fn run_all(arbiter: &mut Arbiter, frames: &[ContactFrame]) -> Vec<FrameDecision> {
    frames
        .iter()
        .map(|frame| arbiter.frame(frame).expect("frame must be accepted"))
        .collect()
}

fn buttons(decisions: &[FrameDecision]) -> Vec<OutputEvent> {
    decisions
        .iter()
        .flat_map(|d| d.events.iter())
        .filter(|e| matches!(e, OutputEvent::ButtonDown(_) | OutputEvent::ButtonUp(_)))
        .cloned()
        .collect()
}

fn moves(decisions: &[FrameDecision]) -> Vec<(f32, f32)> {
    decisions
        .iter()
        .flat_map(|d| d.events.iter())
        .filter_map(|e| match e {
            OutputEvent::PointerMove { dx, dy } => Some((dx.as_px(), dy.as_px())),
            _ => None,
        })
        .collect()
}

#[test]
fn tap_config_is_public_and_validated() {
    // Zero durations are rejected.
    assert_eq!(
        TapConfig::new(true, false, false, Duration::ZERO, mm(1.0), dur(100)),
        Err(TapConfigError::ZeroDuration("max_tap_duration"))
    );
    // Non-positive movement is rejected.
    assert_eq!(
        TapConfig::new(true, false, false, dur(100), mm(0.0), dur(100)),
        Err(TapConfigError::NonPositiveMovement(mm(0.0)))
    );
    // Impossible feature combinations are rejected.
    assert_eq!(
        TapConfig::new(false, true, false, dur(100), mm(1.0), dur(100)),
        Err(TapConfigError::TapAndDragRequiresTap)
    );
    assert_eq!(
        TapConfig::new(true, false, true, dur(100), mm(1.0), dur(100)),
        Err(TapConfigError::DragLockRequiresTapAndDrag)
    );
    // Tapping is disabled unless a validated tap configuration is supplied.
    assert!(cfg().tap_config().is_none());
    assert!(!cfg().is_tap_enabled());
    assert!(cfg().with_tap(tap_cfg()).is_tap_enabled());
}

#[test]
fn public_tap_sequence_with_observable_phase() {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d0 = arbiter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d0.events.is_empty());
    assert_eq!(d0.tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::FirstTapCandidate);
    // The release frame emits exactly down then up and opens the follow-up
    // window.
    let d1 = arbiter
        .frame(&frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d1.events, vec![down(), up()]);
    assert_eq!(d1.tap_drag_phase_after, TapDragPhase::FollowUpWindow);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::FollowUpWindow);
    assert!(!arbiter.is_left_held());
}

#[test]
fn public_tap_and_drag_with_lock_continues_and_unlocks() {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d = run_all(
        &mut arbiter,
        &[
            // first tap
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            // follow-up drag
            frame(
                2,
                2,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 0, ContactState::Active, 11.0, 10.0)],
                false,
                false,
            ),
            // lift -> locked
            frame(
                4,
                4,
                vec![contact(2, 0, ContactState::Ended, 11.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![down(), up(), down()]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    assert!(arbiter.is_synthetic_left_held());
    assert!(arbiter.is_left_held());

    // Reposition: a new contact continues the drag without a new down.
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                5,
                5,
                vec![contact(3, 0, ContactState::Began, 20.0, 20.0)],
                false,
                false,
            ),
            frame(
                6,
                6,
                vec![contact(3, 0, ContactState::Active, 21.0, 20.0)],
                false,
                false,
            ),
            frame(
                7,
                7,
                vec![contact(3, 0, ContactState::Ended, 21.0, 20.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(d[1].events, vec![move_event(10.0, 0.0)]);
    assert!(buttons(&d).is_empty());
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);

    // A qualifying locked tap unlocks with exactly one up.
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                8,
                8,
                vec![contact(4, 0, ContactState::Began, 30.0, 30.0)],
                false,
                false,
            ),
            frame(
                9,
                9,
                vec![contact(4, 0, ContactState::Ended, 30.1, 30.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![up()]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
    assert!(!arbiter.is_synthetic_left_held());
    assert!(!arbiter.is_left_held());
}

#[test]
fn public_physical_competition_in_tap_candidate() {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            // physical press cancels the tap policy (no synthetic click later)
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Active, 0.2, 0.0)],
                true,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(1, 0, ContactState::Ended, 0.2, 0.0)],
                true,
                false,
            ),
            frame(3, 3, vec![], false, false),
        ],
    );
    // Only the physical click; no synthetic tap click.
    assert_eq!(buttons(&d), vec![down(), up()]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
    assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Idle);
}

#[test]
fn public_physical_arbitration_during_lock() {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 0, ContactState::Active, 11.0, 10.0)],
                false,
                false,
            ),
            frame(
                4,
                4,
                vec![contact(2, 0, ContactState::Ended, 11.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert!(arbiter.is_synthetic_left_held());
    // A physical press while the lock holds synthetic left: no duplicate down.
    let d = run_all(&mut arbiter, &[frame(5, 5, vec![], true, false)]);
    assert!(d[0].events.is_empty());
    assert!(arbiter.is_physical_left_held());
    // Physical release while the lock still holds: no up (aggregate stays held).
    let d = run_all(&mut arbiter, &[frame(6, 6, vec![], false, false)]);
    assert!(d[0].events.is_empty());
    assert!(arbiter.is_left_held());
    // release_all ends the aggregate with exactly one up and resets.
    assert_eq!(arbiter.release_all(), vec![up()]);
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
}

#[test]
fn public_decisions_serialize_with_tap_phase() {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
        ],
    );
    for decision in d {
        let json = serde_json::to_string(&decision).unwrap();
        let decoded: FrameDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, decision);
    }
}

/// A scripted fault-injecting sink with a real held-state model (mirrors the
/// `OutputSink` contract): rejects specific submission indices, records
/// accepted events, and its wrapped `release_all()` clears held state.
struct ScriptedSink {
    events: Vec<OutputEvent>,
    reject_submits: Vec<usize>,
    submits: usize,
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
            releases: 0,
            held_left: false,
        }
    }
}

impl OutputSink for ScriptedSink {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        let index = self.submits;
        self.submits += 1;
        if self.reject_submits.contains(&index) {
            return Err(OutputError::Rejected(event));
        }
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
        self.held_left = false;
        Ok(())
    }
}

#[test]
fn arbiter_sink_rejected_tap_up_after_accepted_down_retries_once() {
    // Submissions: 0 = tap down (accepted), 1 = tap up (rejected).
    let mut adapter = ArbiterSink::new(tap_arbiter_cfg(), ScriptedSink::new(vec![1]));
    adapter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let err = adapter
        .frame(&frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
            false,
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
            assert_eq!(index, 1);
            assert_eq!(accepted_prefix, 1);
            assert_eq!(decision_len, 2);
            assert_eq!(failed_event, up());
            assert!(matches!(primary, OutputError::Rejected(_)));
        }
        other => panic!("expected PartialSubmit, got {other:?}"),
    }
    // The accepted tap down stays delivered-held; cleanup retries the up.
    assert!(adapter.arbiter().is_left_held());
    assert!(adapter.sink().held_left);
    assert!(adapter.is_faulted());
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![down(), up()]);
    assert_eq!(sink.submits, 3); // down + rejected up + retried up
    assert!(!sink.held_left);
}

#[test]
fn arbiter_sink_cleanup_while_drag_locked_releases_exactly_once() {
    let mut adapter = ArbiterSink::new(tap_arbiter_cfg(), ScriptedSink::new(vec![usize::MAX]));
    for frame in [
        frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ),
        frame(
            1,
            1,
            vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
            false,
            false,
        ),
        frame(
            2,
            2,
            vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
            false,
            false,
        ),
        frame(
            3,
            3,
            vec![contact(2, 0, ContactState::Active, 11.0, 10.0)],
            false,
            false,
        ),
        frame(
            4,
            4,
            vec![contact(2, 0, ContactState::Ended, 11.0, 10.0)],
            false,
            false,
        ),
    ] {
        adapter.frame(&frame).unwrap();
    }
    assert!(adapter.arbiter().is_synthetic_left_held());
    assert_eq!(
        adapter.arbiter().tap_drag_phase(),
        TapDragPhase::LockedWithoutContact
    );
    // Cleanup while drag-locked: exactly one up, full reset at the
    // acknowledgement boundary, and no lost release.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
    assert_eq!(
        sink.events,
        vec![down(), up(), down(), move_event(10.0, 0.0), up()]
    );
    assert_eq!(sink.releases, 1);
    assert!(!sink.held_left);
}

// ----------------------------------------------------------------------
// M8 review R1–R4 public-contract regressions
// ----------------------------------------------------------------------

/// Drives an arbiter into sticky drag lock (synthetic left held, no contact).
fn locked_arbiter() -> Arbiter {
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 0, ContactState::Active, 11.0, 10.0)],
                false,
                false,
            ),
            frame(
                4,
                4,
                vec![contact(2, 0, ContactState::Ended, 11.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    assert!(arbiter.is_synthetic_left_held());
    arbiter
}

#[test]
fn public_final_ended_pointer_commit_produces_no_synthetic_click() {
    // R1: a first-tap candidate that first crosses the M7 motion threshold
    // (1 mm) in its final Ended frame, with the tap movement limit (2 mm)
    // wider than the pointer threshold and the final displacement exactly at
    // the threshold, emits the pointer move and NO synthetic button pair.
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Active, 0.8, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(1, 0, ContactState::Ended, 1.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(moves(&d), vec![(10.0, 0.0)]);
    assert!(buttons(&d).is_empty());
    assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::Idle);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
}

#[test]
fn public_final_ended_tap_drag_commit_enters_lock_without_up() {
    // R1: a tap-and-drag contact that first crosses the pointer threshold in
    // its final Ended frame commits as a real drag: with sticky drag lock the
    // lift enters locked-without-contact and emits no up.
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 0, ContactState::Active, 10.8, 10.0)],
                false,
                false,
            ),
            // Crosses the 1 mm threshold exactly in the final Ended frame.
            frame(
                4,
                4,
                vec![contact(2, 0, ContactState::Ended, 11.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![down(), up(), down()]);
    assert_eq!(moves(&d), vec![(10.0, 0.0)]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    assert!(arbiter.is_synthetic_left_held());
    assert!(arbiter.is_left_held());
}

#[test]
fn public_final_ended_locked_continuation_remains_locked() {
    // R1: a locked continuation that first crosses the pointer threshold in
    // its final Ended frame remains locked (no unlock tap) even though the
    // tap movement limit (2 mm) is wider than the pointer threshold (1 mm).
    let mut arbiter = locked_arbiter();
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                5,
                5,
                vec![contact(3, 0, ContactState::Began, 20.0, 20.0)],
                false,
                false,
            ),
            frame(
                6,
                6,
                vec![contact(3, 0, ContactState::Active, 20.8, 20.0)],
                false,
                false,
            ),
            frame(
                7,
                7,
                vec![contact(3, 0, ContactState::Ended, 21.0, 20.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(moves(&d), vec![(10.0, 0.0)]);
    assert!(buttons(&d).is_empty());
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    assert!(arbiter.is_synthetic_left_held());
    assert!(arbiter.is_left_held());
}

#[test]
fn public_discontinuity_plus_simultaneous_physical_release_emits_single_up() {
    // R2 stateful regression: enter sticky synthetic lock; press physical
    // left while synthetic remains held (no duplicate down); process one
    // discontinuity=true frame with physical left now false; require exactly
    // one aggregate ButtonUp, no panic, both sources false, lock cancelled,
    // and repeated cleanup/release producing no unmatched up.
    let mut arbiter = locked_arbiter();
    let d = run_all(&mut arbiter, &[frame(5, 5, vec![], true, false)]);
    assert!(
        d[0].events.is_empty(),
        "no duplicate down while synthetic holds"
    );
    assert!(arbiter.is_physical_left_held());
    assert!(arbiter.is_synthetic_left_held());
    let d = run_all(&mut arbiter, &[frame(6, 6, vec![], false, true)]);
    assert_eq!(buttons(&d), vec![up()]);
    assert!(!arbiter.is_synthetic_left_held());
    assert!(!arbiter.is_physical_left_held());
    assert!(!arbiter.is_left_held());
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Cancelled);
    let d = run_all(&mut arbiter, &[frame(7, 7, vec![], false, false)]);
    assert!(d[0].events.is_empty());
    assert_eq!(arbiter.release_all(), Vec::<OutputEvent>::new());
}

#[test]
fn public_discontinuity_began_cannot_seed_tap_or_tap_and_drag() {
    // R3: a Began contact on a discontinuity frame (fresh arbiter, and one
    // arriving inside an open follow-up window) must not seed a tap click or
    // an immediate tap-and-drag down; M7 pointer re-anchoring is preserved;
    // a later genuinely new Began starts tap policy normally.
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    // Fresh arbiter: discontinuity + Began, then a quick small Ended.
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 5.0, 5.0)],
                false,
                true,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 5.1, 5.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
    assert!(
        buttons(&d).is_empty(),
        "discontinuous contact must not click"
    );
    // Open follow-up window: discontinuity + Began must not start tap-and-drag.
    let mut arbiter = Arbiter::new(tap_arbiter_cfg());
    run_all(
        &mut arbiter,
        &[
            frame(
                0,
                0,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::FollowUpWindow);
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                2,
                2,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                true,
            ),
            frame(
                3,
                3,
                vec![contact(2, 0, ContactState::Ended, 10.1, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert!(d[0].events.is_empty(), "no immediate tap-and-drag down");
    assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
    assert!(buttons(&d).is_empty());
    // A later genuinely new Began starts tap policy normally.
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                4,
                4,
                vec![contact(3, 0, ContactState::Began, 20.0, 20.0)],
                false,
                false,
            ),
            frame(
                5,
                5,
                vec![contact(3, 0, ContactState::Ended, 20.1, 20.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
    assert_eq!(buttons(&d), vec![down(), up()]);
}

#[test]
fn public_follow_up_near_u64_max_boundaries_use_checked_elapsed() {
    // R4: follow-up expiry near u64::MAX uses checked elapsed semantics —
    // equality with the configured gap is accepted, strictly greater
    // expires, and a nominal deadline that would overflow u64::MAX is never
    // converted into a different state transition.
    let gap = Duration::from_nanos(500);
    let cfg_near_max =
        cfg().with_tap(TapConfig::new(true, true, true, dur(500), mm(2.0), gap).unwrap());
    let completed = u64::MAX - 1000;

    // Equality: follow-up Began exactly `gap` after the completed tap.
    let mut arbiter = Arbiter::new(cfg_near_max.clone());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                u64::MAX - 1010,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                completed,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                u64::MAX - 500,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![down(), up()]);
    assert!(d[2].events.is_empty());
    assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);

    // Strictly greater: one ns past the deadline closes the window.
    let mut arbiter = Arbiter::new(cfg_near_max.clone());
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                u64::MAX - 1010,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                completed,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                u64::MAX - 499,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![down(), up()]);
    assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);

    // Deadline overflow: completed + gap would exceed u64::MAX; the checked
    // elapsed comparison keeps the window open (elapsed 1000 <= 2000).
    let cfg_overflow = cfg().with_tap(
        TapConfig::new(
            true,
            true,
            true,
            dur(500),
            mm(2.0),
            Duration::from_nanos(2000),
        )
        .unwrap(),
    );
    let mut arbiter = Arbiter::new(cfg_overflow);
    let d = run_all(
        &mut arbiter,
        &[
            frame(
                0,
                u64::MAX - 1010,
                vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
                false,
                false,
            ),
            frame(
                1,
                completed,
                vec![contact(1, 0, ContactState::Ended, 0.1, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                u64::MAX,
                vec![contact(2, 0, ContactState::Began, 10.0, 10.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![down(), up()]);
    assert!(d[2].events.is_empty());
    assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);
}
