//! Public-contract tests for the M9 two-finger scroll / secondary-tap /
//! buttonpad physical-secondary-click policy.
//!
//! These tests use only the crate's public API: the validated
//! [`TwoFingerConfig`], the observable [`TwoFingerPhase`] (on
//! `FrameDecision` and `Arbiter`), the aggregate right-button arbitration
//! (synthetic secondary tap, latched buttonpad press, physical right), the
//! `ScrollBegin → ScrollDelta* → ScrollEnd` lifecycle with explicit natural
//! direction, and the `ArbiterSink` delivery-aware adapter for right/scroll
//! events with a fault-injecting fake sink. No real output sink is ever
//! instantiated.

use std::time::Duration;

use touchpad_core::{
    Arbiter, ArbiterConfig, ArbiterSink, ArbiterSinkError, Contact, ContactFrame, ContactState,
    FrameDecision, Lifecycle, LogicalPixels, LogicalPixelsPerMm, Millimeters, Monotonic,
    MouseButton, OutputError, OutputEvent, OutputSink, PhysicalButtons, TwoFingerConfig,
    TwoFingerConfigError, TwoFingerPhase,
};

fn mm(x: f32) -> Millimeters {
    Millimeters::try_new(x).unwrap()
}

fn px(x: f32) -> LogicalPixels {
    LogicalPixels::try_new(x).unwrap()
}

fn dur(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn cfg() -> ArbiterConfig {
    ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(10.0).unwrap()).unwrap()
}

/// Default M9 test two-finger config: scroll enabled (natural), ppm 10,
/// 0.5 mm scroll commit threshold, secondary tap enabled, buttonpad
/// two-finger physical click enabled, 500 ms tap duration, 2 mm tap movement
/// limit.
fn two_cfg() -> TwoFingerConfig {
    TwoFingerConfig::new(
        true,
        true,
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
        mm(0.5),
        true,
        true,
        dur(500),
        mm(2.0),
    )
    .unwrap()
}

fn two_arbiter_cfg() -> ArbiterConfig {
    cfg().with_two_finger(two_cfg())
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
    frame_buttons(sequence, ts, contacts, left, false, discontinuity)
}

/// A frame with independent physical left/right button state.
fn frame_buttons(
    sequence: u64,
    ts: u64,
    contacts: Vec<Contact>,
    left: bool,
    right: bool,
    discontinuity: bool,
) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ts),
        sequence,
        discontinuity,
        contacts,
        physical_buttons: PhysicalButtons::new(left, right, false),
        diagnostics: vec![],
    }
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

fn scroll_deltas(decisions: &[FrameDecision]) -> Vec<(f32, f32)> {
    decisions
        .iter()
        .flat_map(|d| d.events.iter())
        .filter_map(|e| match e {
            OutputEvent::ScrollDelta { dx, dy } => Some((dx.as_px(), dy.as_px())),
            _ => None,
        })
        .collect()
}

fn right_down() -> OutputEvent {
    OutputEvent::ButtonDown(MouseButton::Right)
}

fn right_up() -> OutputEvent {
    OutputEvent::ButtonUp(MouseButton::Right)
}

#[test]
fn two_finger_config_is_public_and_validated() {
    // Non-positive scroll commit threshold is rejected.
    assert_eq!(
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.0),
            true,
            true,
            dur(100),
            mm(1.0),
        ),
        Err(TwoFingerConfigError::NonPositiveScrollThreshold(mm(0.0)))
    );
    // Zero secondary-tap duration is rejected.
    assert_eq!(
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true,
            true,
            Duration::ZERO,
            mm(1.0),
        ),
        Err(TwoFingerConfigError::ZeroDuration(
            "max_secondary_tap_duration"
        ))
    );
    // Non-positive secondary-tap movement is rejected.
    assert_eq!(
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true,
            true,
            dur(100),
            mm(-1.0),
        ),
        Err(TwoFingerConfigError::NonPositiveMovement(mm(-1.0)))
    );
    // The two-finger family is disabled unless a validated config is
    // attached.
    assert!(cfg().two_finger_config().is_none());
    assert!(!cfg().is_two_finger_enabled());
    assert!(cfg().with_two_finger(two_cfg()).is_two_finger_enabled());
}

#[test]
fn public_two_finger_scroll_with_observable_phase_and_natural_direction() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
            // The frame where the second valid contact appears anchors the
            // candidate: no pointer, button, or scroll event leaks.
            frame(
                1,
                1,
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Centroid moves +0.5 mm in x and +0.5 mm in y: equality at the
            // threshold commits; natural=true keeps the sign on both axes.
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.5, 0.5),
                    contact(2, 1, ContactState::Active, 10.5, 0.5),
                ],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Ended, 0.5, 0.5),
                    contact(2, 1, ContactState::Active, 10.5, 0.5),
                ],
                false,
                false,
            ),
        ],
    );
    assert!(d[0].events.is_empty());
    assert_eq!(d[0].two_finger_phase_after, TwoFingerPhase::Idle);
    assert!(d[1].events.is_empty());
    assert_eq!(d[1].two_finger_phase_after, TwoFingerPhase::Candidate);
    // Commit: ScrollBegin, then the accumulated (5, 5) px exactly once.
    assert_eq!(d[2].events[0], OutputEvent::ScrollBegin);
    assert_eq!(scroll_deltas(&d), vec![(5.0, 5.0)]);
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    // The committed scroll ends by release with exactly one ScrollEnd and no
    // secondary tap.
    assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd]);
    assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Finished);
    assert_eq!(arbiter.two_finger_phase(), TwoFingerPhase::Finished);
    assert!(buttons(&d).is_empty());
}

#[test]
fn public_natural_direction_negates_both_axes() {
    let cfg_non_natural = cfg().with_two_finger(
        TwoFingerConfig::new(
            true,
            false, // non-natural: each axis is negated
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true,
            true,
            dur(500),
            mm(2.0),
        )
        .unwrap(),
    );
    let mut arbiter = Arbiter::new(cfg_non_natural);
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 1.0, 1.0),
                    contact(2, 1, ContactState::Active, 11.0, 1.0),
                ],
                false,
                false,
            ),
        ],
    );
    // Centroid moved +1.0 mm on each axis -> natural=false negates: (-10, -10).
    assert_eq!(scroll_deltas(&d), vec![(-10.0, -10.0)]);
}

#[test]
fn public_two_finger_secondary_tap_emits_one_right_click_pair() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // The first boundary that ends the exactly-two interaction fires
            // exactly ButtonDown(Right), ButtonUp(Right) in order.
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Ended, 0.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 1, ContactState::Ended, 10.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Finished);
    assert!(!arbiter.is_right_held());
    // No pointer or scroll output interleaved with the tap.
    assert!(scroll_deltas(&d).is_empty());
}

#[test]
fn public_physical_two_finger_click_latches_right_and_stays_latched() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Physical-left press with exactly two fingers: latched to Right.
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                true,
                false,
            ),
            // Finger count changes while held: the latch never remaps.
            frame(
                3,
                3,
                vec![contact(2, 1, ContactState::Active, 10.0, 0.0)],
                true,
                false,
            ),
            frame(4, 4, vec![], true, false),
            frame(
                5,
                5,
                vec![
                    contact(3, 0, ContactState::Began, 20.0, 20.0),
                    contact(4, 1, ContactState::Began, 30.0, 20.0),
                ],
                true,
                false,
            ),
            // Matching physical release: exactly one Right up.
            frame(
                6,
                6,
                vec![
                    contact(3, 0, ContactState::Active, 20.0, 20.0),
                    contact(4, 1, ContactState::Active, 30.0, 20.0),
                ],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    assert_eq!(
        d[2].two_finger_phase_after,
        TwoFingerPhase::PhysicalSecondaryClickHeld
    );
    assert!(!arbiter.is_latched_right_held());
    assert!(!arbiter.is_left_held());
    assert!(!arbiter.is_right_held());
}

#[test]
fn public_one_finger_physical_click_remains_left() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![contact(1, 0, ContactState::Active, 0.0, 0.0)],
                true,
                false,
            ),
            frame(
                2,
                2,
                vec![contact(1, 0, ContactState::Active, 0.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(
        buttons(&d),
        vec![
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::ButtonUp(MouseButton::Left),
        ]
    );
    assert!(!arbiter.is_right_held());
}

#[test]
fn public_release_all_closes_scroll_and_releases_right_exactly_once() {
    // Scroll open: release_all emits exactly one ScrollEnd and resets.
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 1.0, 0.0),
                    contact(2, 1, ContactState::Active, 11.0, 0.0),
                ],
                false,
                false,
            ),
        ],
    );
    assert!(arbiter.is_scroll_open());
    assert_eq!(arbiter.release_all(), vec![OutputEvent::ScrollEnd]);
    assert_eq!(arbiter.release_all(), Vec::<OutputEvent>::new()); // idempotent
    assert_eq!(arbiter.two_finger_phase(), TwoFingerPhase::Idle);
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);

    // Latched right held: release_all emits exactly one ButtonUp(Right).
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                true,
                false,
            ),
        ],
    );
    assert!(arbiter.is_right_held());
    assert_eq!(arbiter.release_all(), vec![right_up()]);
    assert_eq!(arbiter.release_all(), Vec::<OutputEvent>::new());
    // A fresh interaction after the reset.
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Ended, 0.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![right_down(), right_up()]);
}

#[test]
fn public_arbiter_sink_fault_rejected_scroll_end_after_accepted_begin_retries() {
    // A scripted fault-injecting sink with a real held/open-state model.
    struct ScriptedSink {
        events: Vec<OutputEvent>,
        reject_submits: Vec<usize>,
        submits: usize,
        held_right: bool,
        scroll_open: bool,
    }
    impl OutputSink for ScriptedSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            let index = self.submits;
            self.submits += 1;
            if self.reject_submits.contains(&index) {
                return Err(OutputError::Rejected(event));
            }
            match &event {
                OutputEvent::ButtonDown(MouseButton::Right) => self.held_right = true,
                OutputEvent::ButtonUp(MouseButton::Right) => self.held_right = false,
                OutputEvent::ScrollBegin => self.scroll_open = true,
                OutputEvent::ScrollEnd => self.scroll_open = false,
                _ => {}
            }
            self.events.push(event);
            Ok(())
        }
        fn release_all(&mut self) -> Result<(), OutputError> {
            self.held_right = false;
            self.scroll_open = false;
            Ok(())
        }
    }

    // Submissions: 0 = ScrollBegin, 1 = ScrollDelta, 2 = ScrollEnd (rejected).
    let mut adapter = ArbiterSink::new(
        two_arbiter_cfg(),
        ScriptedSink {
            events: Vec::new(),
            reject_submits: vec![2],
            submits: 0,
            held_right: false,
            scroll_open: false,
        },
    );
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
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap();
    adapter
        .frame(&frame(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Active, 1.0, 0.0),
                contact(2, 1, ContactState::Active, 11.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap();
    // End frame [ScrollEnd] rejected: the open lifecycle stays owed.
    let err = adapter
        .frame(&frame(
            3,
            3,
            vec![
                contact(1, 0, ContactState::Ended, 1.0, 0.0),
                contact(2, 1, ContactState::Active, 11.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::PartialSubmit {
            index: 0,
            accepted_prefix: 0,
            decision_len: 1,
            ..
        }
    ));
    assert!(adapter.arbiter().is_scroll_open());
    assert!(adapter.sink().scroll_open);
    assert!(adapter.is_faulted());
    // Cleanup retries the ScrollEnd exactly once.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(
        sink.events,
        vec![
            OutputEvent::ScrollBegin,
            OutputEvent::ScrollDelta {
                dx: px(10.0),
                dy: px(0.0)
            },
            OutputEvent::ScrollEnd,
        ]
    );
    assert!(!sink.scroll_open);
}

#[test]
fn public_arbiter_sink_fault_rejected_right_up_after_accepted_down_retries() {
    struct ScriptedSink {
        events: Vec<OutputEvent>,
        reject_submits: Vec<usize>,
        submits: usize,
        held_right: bool,
    }
    impl OutputSink for ScriptedSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            let index = self.submits;
            self.submits += 1;
            if self.reject_submits.contains(&index) {
                return Err(OutputError::Rejected(event));
            }
            match &event {
                OutputEvent::ButtonDown(MouseButton::Right) => self.held_right = true,
                OutputEvent::ButtonUp(MouseButton::Right) => self.held_right = false,
                _ => {}
            }
            self.events.push(event);
            Ok(())
        }
        fn release_all(&mut self) -> Result<(), OutputError> {
            self.held_right = false;
            Ok(())
        }
    }

    // Secondary tap [RightDown, RightUp]: down (sub 0) accepted, up (sub 1)
    // rejected -> the right stays held and cleanup retries the up once.
    let mut adapter = ArbiterSink::new(
        two_arbiter_cfg(),
        ScriptedSink {
            events: Vec::new(),
            reject_submits: vec![1],
            submits: 0,
            held_right: false,
        },
    );
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
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap();
    assert!(err.events.is_empty());
    let err = adapter
        .frame(&frame(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Ended, 0.0, 0.0),
                contact(2, 1, ContactState::Active, 10.0, 0.0),
            ],
            false,
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
    assert!(adapter.arbiter().is_right_held());
    assert!(adapter.sink().held_right);
    assert!(adapter.is_faulted());
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, vec![right_down(), right_up()]);
    assert!(!sink.held_right);
}

#[test]
fn public_arbiter_sink_rejected_right_down_owes_no_up() {
    struct ScriptedSink {
        events: Vec<OutputEvent>,
        reject_submits: Vec<usize>,
        submits: usize,
        held_right: bool,
    }
    impl OutputSink for ScriptedSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            let index = self.submits;
            self.submits += 1;
            if self.reject_submits.contains(&index) {
                return Err(OutputError::Rejected(event));
            }
            match &event {
                OutputEvent::ButtonDown(MouseButton::Right) => self.held_right = true,
                OutputEvent::ButtonUp(MouseButton::Right) => self.held_right = false,
                _ => {}
            }
            self.events.push(event);
            Ok(())
        }
        fn release_all(&mut self) -> Result<(), OutputError> {
            self.held_right = false;
            Ok(())
        }
    }

    let mut adapter = ArbiterSink::new(
        two_arbiter_cfg(),
        ScriptedSink {
            events: Vec::new(),
            reject_submits: vec![0],
            submits: 0,
            held_right: false,
        },
    );
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
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap();
    // The tap [RightDown, RightUp]: the down (sub 0) is rejected -> nothing
    // was delivered; cleanup owes no up.
    let err = adapter
        .frame(&frame(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Ended, 0.0, 0.0),
                contact(2, 1, ContactState::Active, 10.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::PartialSubmit {
            index: 0,
            accepted_prefix: 0,
            decision_len: 2,
            ..
        }
    ));
    assert!(!adapter.arbiter().is_right_held());
    assert!(adapter.is_faulted());
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.events, Vec::<OutputEvent>::new()); // no unmatched up
    assert!(!sink.held_right);
}

#[test]
fn public_decisions_serialize_with_two_finger_phase() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 1.0, 0.0),
                    contact(2, 1, ContactState::Active, 11.0, 0.0),
                ],
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

// ------------------------------------------------------------------
// M9 review R1–R6 public regressions (reviews/M9_REVIEW.md, binding)
// ------------------------------------------------------------------

/// R1 (public): with `scroll_enabled=false`, centroid motion far past the
/// scroll commit threshold never opens or emits a scroll lifecycle, while a
/// qualifying quick two-finger lift still emits the secondary click pair.
#[test]
fn public_scroll_disabled_never_opens_scroll_lifecycle() {
    let cfg_no_scroll = cfg().with_two_finger(
        TwoFingerConfig::new(
            false, // scroll disabled
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true, // secondary tap enabled
            false,
            dur(500),
            mm(2.0),
        )
        .unwrap(),
    );
    let mut arbiter = Arbiter::new(cfg_no_scroll);
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // 1.0 mm centroid movement >= 0.5 mm scroll threshold (and within
            // the 2.0 mm tap movement limit): must not commit a scroll.
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 1.0, 0.0),
                    contact(2, 1, ContactState::Active, 11.0, 0.0),
                ],
                false,
                false,
            ),
            // Quick lift: the secondary tap still fires.
            frame(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Ended, 1.0, 0.0),
                    contact(2, 1, ContactState::Active, 11.0, 0.0),
                ],
                false,
                false,
            ),
        ],
    );
    for decision in &d {
        assert!(!decision.events.iter().any(|e| matches!(
            e,
            OutputEvent::ScrollBegin | OutputEvent::ScrollDelta { .. } | OutputEvent::ScrollEnd
        )));
    }
    assert!(!arbiter.is_scroll_open());
    assert_eq!(
        d[2].two_finger_phase_after,
        TwoFingerPhase::Candidate,
        "scroll disabled: the candidate never commits"
    );
    assert_eq!(buttons(&d), vec![right_down(), right_up()]);
}

/// R2 (public): a primary physical-left press begun with one finger followed
/// by a second finger — with physical Left still held at the release boundary
/// — must never synthesize a secondary click in the continuing contact
/// cluster.
#[test]
fn public_physical_left_held_at_release_blocks_secondary_tap() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![contact(1, 0, ContactState::Active, 0.0, 0.0)],
                true,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                true,
                false,
            ),
            // Release boundary with physical Left still held: no Right tap.
            frame(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Ended, 0.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                true,
                false,
            ),
            frame(
                4,
                4,
                vec![contact(2, 1, ContactState::Active, 10.0, 0.0)],
                true,
                false,
            ),
            frame(5, 5, vec![], false, false),
        ],
    );
    assert_eq!(
        buttons(&d),
        vec![
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::ButtonUp(MouseButton::Left),
        ],
        "only the primary physical Left press/release; never a Right tap"
    );
    assert!(!arbiter.is_right_held());
    assert!(!arbiter.is_left_held());
}

/// R2 (public): a committed one-finger pointer interaction followed by a
/// second finger and a quick two-finger release must not synthesize a
/// secondary click — one continuous cluster cannot commit pointer and
/// secondary-tap ownership.
#[test]
fn public_committed_pointer_then_quick_two_finger_release_no_secondary_tap() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
            // The one-finger pointer commits and emits PointerMove.
            frame(
                1,
                1,
                vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 2.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Quick small lift: no secondary tap.
            frame(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Ended, 2.0, 0.0),
                    contact(2, 1, ContactState::Active, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                4,
                4,
                vec![contact(2, 1, ContactState::Ended, 10.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert!(buttons(&d).is_empty());
    assert_eq!(d[2].lifecycle_after, Lifecycle::Cancelled);
}

/// R3 (public): a third-finger cancellation disables secondary tap for the
/// continuing contact cluster even after the third finger lifts and the
/// original two Active contacts stabilize; after all contacts end, a
/// genuinely fresh pair taps normally.
#[test]
fn public_cancellation_disqualifies_cluster_until_fresh_cluster() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Third finger: deterministic cancellation.
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.2, 0.0),
                    contact(2, 1, ContactState::Active, 10.2, 0.0),
                    contact(3, 2, ContactState::Began, 20.0, 20.0),
                ],
                false,
                false,
            ),
            // Third finger lifts: the original pair stabilizes again.
            frame(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Active, 0.2, 0.0),
                    contact(2, 1, ContactState::Active, 10.2, 0.0),
                ],
                false,
                false,
            ),
            // Quick lift: no secondary tap in the disqualified cluster.
            frame(
                4,
                4,
                vec![
                    contact(1, 0, ContactState::Ended, 0.2, 0.0),
                    contact(2, 1, ContactState::Active, 10.2, 0.0),
                ],
                false,
                false,
            ),
            frame(
                5,
                5,
                vec![contact(2, 1, ContactState::Ended, 10.2, 0.0)],
                false,
                false,
            ),
            // Genuinely fresh pair after the drain: taps normally.
            frame(
                6,
                6,
                vec![contact(4, 0, ContactState::Began, 30.0, 30.0)],
                false,
                false,
            ),
            frame(
                7,
                7,
                vec![
                    contact(4, 0, ContactState::Active, 30.0, 30.0),
                    contact(5, 1, ContactState::Began, 40.0, 30.0),
                ],
                false,
                false,
            ),
            frame(
                8,
                8,
                vec![
                    contact(4, 0, ContactState::Ended, 30.0, 30.0),
                    contact(5, 1, ContactState::Active, 40.0, 30.0),
                ],
                false,
                false,
            ),
        ],
    );
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
    assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Candidate);
    assert_eq!(
        buttons(&d),
        vec![right_down(), right_up()],
        "only the fresh-cluster tap fires; the cancelled cluster never taps"
    );
}

/// R4 (public): a physical Right press while a committed scroll is open emits
/// `ScrollEnd` before `ButtonDown(Right)` in the same frame (exact ordering).
#[test]
fn public_physical_right_press_while_scrolling_orders_scroll_end_before_down() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            frame(
                2,
                2,
                vec![
                    contact(1, 0, ContactState::Active, 0.8, 0.0),
                    contact(2, 1, ContactState::Active, 10.8, 0.0),
                ],
                false,
                false,
            ),
            // Physical right press while scrolling.
            frame_buttons(
                3,
                3,
                vec![
                    contact(1, 0, ContactState::Active, 0.9, 0.0),
                    contact(2, 1, ContactState::Active, 10.9, 0.0),
                ],
                false,
                true,
                false,
            ),
        ],
    );
    assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd, right_down()]);
}

/// R6 (public): a member that disappears without a clean, complete `Ended`
/// record cannot synthesize a secondary tap at the below-two boundary.
#[test]
fn public_disappearance_without_ended_cancels_secondary_tap() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Contact 1 vanishes with no Ended record.
            frame(
                2,
                2,
                vec![contact(2, 1, ContactState::Active, 10.0, 0.0)],
                false,
                false,
            ),
            frame(
                3,
                3,
                vec![contact(2, 1, ContactState::Ended, 10.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert!(buttons(&d).is_empty());
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
}

/// R6 (public): clean `Ended` evidence from at least one anchored pair member
/// still qualifies the tap when the other member disappears.
#[test]
fn public_one_clean_ended_pair_member_still_qualifies_tap() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
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
                vec![
                    contact(1, 0, ContactState::Active, 0.0, 0.0),
                    contact(2, 1, ContactState::Began, 10.0, 0.0),
                ],
                false,
                false,
            ),
            // Contact 1 disappears; contact 2 (a pair member) ends cleanly.
            frame(
                2,
                2,
                vec![contact(2, 1, ContactState::Ended, 10.0, 0.0)],
                false,
                false,
            ),
        ],
    );
    assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Finished);
}

/// R5 (public): with `ButtonDown(Left)` and `ButtonDown(Right)` held
/// simultaneously, a failed cleanup reports **both** explicit release
/// failures structurally (`primary` + `others`) and the retry submits exactly
/// the still-owed releases once. The dual-owed state is reached by a
/// legitimate, reachable sequence — simultaneous physical Left and Right
/// holds — not by the now-invalid held-button-plus-open-scroll state (review
/// M9 R7: physical button ownership excludes scroll ownership); scroll
/// cleanup/retry coverage is retained in separate tests.
#[test]
fn public_release_all_reports_both_cleanup_failures() {
    struct ScriptedSink {
        events: Vec<OutputEvent>,
        reject_submits: Vec<usize>,
        submits: usize,
        release_failures_left: usize,
        held_left: bool,
        held_right: bool,
        scroll_open: bool,
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
                OutputEvent::ButtonDown(MouseButton::Right) => self.held_right = true,
                OutputEvent::ButtonUp(MouseButton::Right) => self.held_right = false,
                OutputEvent::ScrollBegin => self.scroll_open = true,
                OutputEvent::ScrollEnd => self.scroll_open = false,
                _ => {}
            }
            self.events.push(event);
            Ok(())
        }
        fn release_all(&mut self) -> Result<(), OutputError> {
            if self.release_failures_left > 0 {
                self.release_failures_left -= 1;
                return Err(OutputError::Io("scripted cleanup failure".to_string()));
            }
            self.held_left = false;
            self.held_right = false;
            self.scroll_open = false;
            Ok(())
        }
    }

    // Submissions: 0 = LeftDown, 1 = PointerMove (left press + commit
    // frame), 2 = RightDown (second physical press while Left is held),
    // 3 = PointerMove rejected (continuation frame), then on cleanup:
    // 4 = LeftUp rejected, 5 = RightUp rejected; wrapped cleanup fails once;
    // retry: 6 = LeftUp, 7 = RightUp accepted; wrapped cleanup succeeds.
    let mut adapter = ArbiterSink::new(
        two_arbiter_cfg(),
        ScriptedSink {
            events: Vec::new(),
            reject_submits: vec![3, 4, 5],
            submits: 0,
            release_failures_left: 1,
            held_left: false,
            held_right: false,
            scroll_open: false,
        },
    );
    adapter
        .frame(&frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Physical left press while the pointer commits: [LeftDown, PointerMove].
    adapter
        .frame(&frame_buttons(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            true,
            false,
            false,
        ))
        .unwrap();
    // Physical right press while Left is still held: [RightDown].
    adapter
        .frame(&frame_buttons(
            2,
            2,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            true,
            true,
            false,
        ))
        .unwrap();
    assert!(adapter.arbiter().is_left_held());
    assert!(adapter.arbiter().is_right_held());
    // Continued motion while both are held: [PointerMove] rejected -> both
    // held buttons stay delivered/owed.
    let err = adapter
        .frame(&frame_buttons(
            3,
            3,
            vec![contact(1, 0, ContactState::Active, 2.5, 0.0)],
            true,
            true,
            false,
        ))
        .unwrap_err();
    assert!(matches!(
        err,
        ArbiterSinkError::PartialSubmit {
            index: 0,
            accepted_prefix: 0,
            decision_len: 1,
            ..
        }
    ));
    assert!(adapter.is_faulted());
    // First cleanup: both explicit releases fail and the wrapped cleanup
    // fails: the structured error reports BOTH explicit failures (R5).
    let err = adapter.release_all().unwrap_err();
    match err {
        ArbiterSinkError::ReleaseFailed {
            primary,
            others,
            cleanup,
        } => {
            assert_eq!(
                primary,
                Some(OutputError::Rejected(OutputEvent::ButtonUp(
                    MouseButton::Left
                )))
            );
            assert_eq!(
                others,
                vec![OutputError::Rejected(OutputEvent::ButtonUp(
                    MouseButton::Right
                ))]
            );
            assert!(cleanup.is_some());
        }
        other => panic!("expected ReleaseFailed, got {other:?}"),
    }
    assert!(adapter.arbiter().is_left_held());
    assert!(adapter.arbiter().is_right_held());
    // Retry: exactly the still-owed releases are submitted once each.
    adapter.release_all().unwrap();
    let (arbiter, sink) = adapter.into_parts();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(sink.submits, 8);
    assert!(!sink.held_left);
    assert!(!sink.held_right);
    assert!(!sink.scroll_open);
    assert_eq!(
        sink.events,
        vec![
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::PointerMove {
                dx: px(20.0),
                dy: px(0.0)
            },
            right_down(),
            OutputEvent::ButtonUp(MouseButton::Left),
            right_up(),
        ]
    );
}

/// R7 (public): a physical Right press held before the two-finger pair forms
/// excludes scroll ownership while held — no candidate anchors, no scroll
/// lifecycle opens during continued motion — and after the clean release the
/// same still-live pair may re-anchor and scroll from a fresh relative
/// anchor. No frame exposes simultaneous physical-button and scroll
/// ownership.
#[test]
fn public_physical_right_held_before_pair_blocks_scroll_until_release() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
    let frames = [
        frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ),
        // Physical right press with one finger: [RightDown].
        frame_buttons(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 0.0, 0.0)],
            false,
            true,
            false,
        ),
        // The second finger appears while Right is held: no candidate.
        frame_buttons(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            true,
            false,
        ),
        // Continued motion past the scroll threshold while held: no scroll.
        frame_buttons(
            3,
            3,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            false,
            true,
            false,
        ),
        // Clean release: [RightUp]; the pair re-anchors.
        frame_buttons(
            4,
            4,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            false,
            false,
            false,
        ),
        // Fresh relative scroll from the post-release anchor works.
        frame_buttons(
            5,
            5,
            vec![
                contact(1, 0, ContactState::Active, 2.5, 0.0),
                contact(2, 1, ContactState::Active, 12.5, 0.0),
            ],
            false,
            false,
            false,
        ),
    ];
    let mut d = Vec::new();
    for frame in &frames {
        d.push(arbiter.frame(frame).expect("frame must be accepted"));
        let button_held = arbiter.is_physical_left_held()
            || arbiter.is_physical_right_held()
            || arbiter.is_latched_right_held();
        assert!(
            !(button_held && arbiter.is_scroll_open()),
            "frame {} exposes simultaneous physical-button and scroll ownership",
            frame.sequence
        );
    }
    assert_eq!(d[1].events, vec![right_down()]);
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Idle);
    assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Idle);
    assert!(d[2].events.is_empty());
    assert!(d[3].events.is_empty());
    assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Candidate);
    assert_eq!(d[4].events, vec![right_up()]);
    assert_eq!(d[5].events[0], OutputEvent::ScrollBegin);
    assert_eq!(scroll_deltas(&d[4..]), vec![(5.0, 0.0)]);
    assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    assert!(!arbiter.is_right_held());
}

/// R7 (public): a physical Left press held before the two-finger pair forms
/// (a primary-left press, not latched) likewise excludes scroll ownership
/// while held; after the clean release the same still-live pair re-anchors
/// and scrolls, and secondary tap stays cluster-disqualified.
#[test]
fn public_physical_left_held_before_pair_blocks_scroll_until_release() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
    let frames = [
        frame(
            0,
            0,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ),
        // Physical left press with one finger: [LeftDown].
        frame_buttons(
            1,
            1,
            vec![contact(1, 0, ContactState::Active, 0.0, 0.0)],
            true,
            false,
            false,
        ),
        // The second finger appears while Left is held: no candidate.
        frame_buttons(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            true,
            false,
            false,
        ),
        // Continued motion while held: no scroll lifecycle.
        frame_buttons(
            3,
            3,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            true,
            false,
            false,
        ),
        // Clean release: [LeftUp]; the pair re-anchors.
        frame_buttons(
            4,
            4,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            false,
            false,
            false,
        ),
        // Fresh relative scroll from the post-release anchor works.
        frame_buttons(
            5,
            5,
            vec![
                contact(1, 0, ContactState::Active, 2.5, 0.0),
                contact(2, 1, ContactState::Active, 12.5, 0.0),
            ],
            false,
            false,
            false,
        ),
    ];
    let mut d = Vec::new();
    for frame in &frames {
        d.push(arbiter.frame(frame).expect("frame must be accepted"));
        let button_held = arbiter.is_physical_left_held()
            || arbiter.is_physical_right_held()
            || arbiter.is_latched_right_held();
        assert!(
            !(button_held && arbiter.is_scroll_open()),
            "frame {} exposes simultaneous physical-button and scroll ownership",
            frame.sequence
        );
    }
    assert_eq!(
        d[1].events,
        vec![OutputEvent::ButtonDown(MouseButton::Left)]
    );
    assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Idle);
    assert!(d[2].events.is_empty());
    assert!(d[3].events.is_empty());
    assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Candidate);
    assert_eq!(d[4].events, vec![OutputEvent::ButtonUp(MouseButton::Left)]);
    assert_eq!(d[5].events[0], OutputEvent::ScrollBegin);
    assert_eq!(scroll_deltas(&d[4..]), vec![(5.0, 0.0)]);
    assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    assert!(!arbiter.is_left_held());
}

/// R7 (public): a physical Right press during a committed scroll emits
/// `[ScrollEnd, RightDown]` in the same frame (R4 order) and does not
/// re-anchor a candidate while held; continued motion emits no scroll, and
/// after the clean release the same still-live pair re-anchors and scrolls
/// from a fresh relative anchor.
#[test]
fn public_physical_right_press_during_scroll_blocks_reopen_until_release() {
    let mut arbiter = Arbiter::new(two_arbiter_cfg());
    let frames = [
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
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            false,
        ),
        // 0.8 mm centroid movement >= 0.5 mm threshold: commit.
        frame(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Active, 0.8, 0.0),
                contact(2, 1, ContactState::Active, 10.8, 0.0),
            ],
            false,
            false,
        ),
        // Physical right press while scrolling: [ScrollEnd, RightDown]; no
        // re-anchor while held.
        frame_buttons(
            3,
            3,
            vec![
                contact(1, 0, ContactState::Active, 0.9, 0.0),
                contact(2, 1, ContactState::Active, 10.9, 0.0),
            ],
            false,
            true,
            false,
        ),
        // Continued motion while held: no scroll re-opens.
        frame_buttons(
            4,
            4,
            vec![
                contact(1, 0, ContactState::Active, 1.5, 0.0),
                contact(2, 1, ContactState::Active, 11.5, 0.0),
            ],
            false,
            true,
            false,
        ),
        // Clean release: [RightUp]; the pair re-anchors.
        frame_buttons(
            5,
            5,
            vec![
                contact(1, 0, ContactState::Active, 1.5, 0.0),
                contact(2, 1, ContactState::Active, 11.5, 0.0),
            ],
            false,
            false,
            false,
        ),
        // Fresh relative scroll from the post-release anchor works.
        frame_buttons(
            6,
            6,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            false,
            false,
            false,
        ),
    ];
    let mut d = Vec::new();
    for frame in &frames {
        d.push(arbiter.frame(frame).expect("frame must be accepted"));
        let button_held = arbiter.is_physical_left_held()
            || arbiter.is_physical_right_held()
            || arbiter.is_latched_right_held();
        assert!(
            !(button_held && arbiter.is_scroll_open()),
            "frame {} exposes simultaneous physical-button and scroll ownership",
            frame.sequence
        );
    }
    assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd, right_down()]);
    assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Cancelled);
    assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Cancelled);
    assert!(d[4].events.is_empty());
    assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::Candidate);
    assert_eq!(d[5].events, vec![right_up()]);
    assert_eq!(d[6].events[0], OutputEvent::ScrollBegin);
    assert_eq!(scroll_deltas(&d), vec![(8.0, 0.0), (5.0, 0.0)]);
    assert_eq!(d[6].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    assert!(!arbiter.is_right_held());
}

/// R7 (public): a physical Left press during a committed scroll with the
/// buttonpad physical-click policy disabled is a normal left press (not a
/// latch) and follows the same non-latched exclusion: same-frame
/// `[ScrollEnd, LeftDown]`, no re-anchor while held, and after the clean
/// release the pair re-anchors and scrolls from a fresh relative anchor.
#[test]
fn public_physical_left_press_during_scroll_blocks_reopen_until_release() {
    let cfg_no_click = cfg().with_two_finger(
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true,
            false, // buttonpad two-finger physical click disabled
            dur(500),
            mm(2.0),
        )
        .unwrap(),
    );
    let mut arbiter = Arbiter::new(cfg_no_click);
    let frames = [
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
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 10.0, 0.0),
            ],
            false,
            false,
        ),
        // 0.8 mm centroid movement >= 0.5 mm threshold: commit.
        frame(
            2,
            2,
            vec![
                contact(1, 0, ContactState::Active, 0.8, 0.0),
                contact(2, 1, ContactState::Active, 10.8, 0.0),
            ],
            false,
            false,
        ),
        // Physical left press while scrolling (policy disabled -> a normal
        // left press): [ScrollEnd, LeftDown]; no re-anchor while held.
        frame_buttons(
            3,
            3,
            vec![
                contact(1, 0, ContactState::Active, 0.9, 0.0),
                contact(2, 1, ContactState::Active, 10.9, 0.0),
            ],
            true,
            false,
            false,
        ),
        // Continued motion while held: no scroll re-opens.
        frame_buttons(
            4,
            4,
            vec![
                contact(1, 0, ContactState::Active, 1.5, 0.0),
                contact(2, 1, ContactState::Active, 11.5, 0.0),
            ],
            true,
            false,
            false,
        ),
        // Clean release: [LeftUp]; the pair re-anchors.
        frame_buttons(
            5,
            5,
            vec![
                contact(1, 0, ContactState::Active, 1.5, 0.0),
                contact(2, 1, ContactState::Active, 11.5, 0.0),
            ],
            false,
            false,
            false,
        ),
        // Fresh relative scroll from the post-release anchor works.
        frame_buttons(
            6,
            6,
            vec![
                contact(1, 0, ContactState::Active, 2.0, 0.0),
                contact(2, 1, ContactState::Active, 12.0, 0.0),
            ],
            false,
            false,
            false,
        ),
    ];
    let mut d = Vec::new();
    for frame in &frames {
        d.push(arbiter.frame(frame).expect("frame must be accepted"));
        let button_held = arbiter.is_physical_left_held()
            || arbiter.is_physical_right_held()
            || arbiter.is_latched_right_held();
        assert!(
            !(button_held && arbiter.is_scroll_open()),
            "frame {} exposes simultaneous physical-button and scroll ownership",
            frame.sequence
        );
    }
    assert_eq!(
        d[3].events,
        vec![
            OutputEvent::ScrollEnd,
            OutputEvent::ButtonDown(MouseButton::Left)
        ]
    );
    assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Cancelled);
    assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Cancelled);
    assert!(d[4].events.is_empty());
    assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::Candidate);
    assert_eq!(d[5].events, vec![OutputEvent::ButtonUp(MouseButton::Left)]);
    assert_eq!(d[6].events[0], OutputEvent::ScrollBegin);
    assert_eq!(scroll_deltas(&d), vec![(8.0, 0.0), (5.0, 0.0)]);
    assert_eq!(d[6].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    assert!(!arbiter.is_left_held());
}
