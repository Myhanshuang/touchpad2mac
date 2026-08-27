//! M8 integration: trace/replay-derived frames and directly synthetic frames
//! exercise the **same** arbiter path for the tap policy.
//!
//! Replay drives the exact M3 Type-B decoder used by live input (there is no
//! second decoder), producing `ContactFrame`s; synthetic frames are built by
//! hand with identical content. Both are fed to the M8 `Arbiter` (with a
//! validated tap configuration), and the per-frame decisions must be
//! identical — proving the tap/tap-and-drag policy is a pure, deterministic
//! function of the normalized frame stream regardless of how the frames were
//! produced.

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use touchpad_core::{
    raw_axis_position_to_mm_with_resolution, Arbiter, ArbiterConfig, AxisInfo, Contact,
    ContactFrame, ContactState, Diagnostic, FrameDecision, Lifecycle, LifecycleTransition,
    LogicalPixelsPerMm, Millimeters, Monotonic, MouseButton, OutputEvent, PhysicalButtons, RawAxis,
    TapConfig, TapDragPhase,
};
use touchpad_linux::{RecordingFrameSink, TypeBDecoder};
use touchpad_trace::ReplayDriver;

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../touchpad-trace/tests/fixtures")
        .join(format!("{name}.jsonl"))
}

fn mm(raw: i32) -> Millimeters {
    let info = AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100));
    raw_axis_position_to_mm_with_resolution(RawAxis::new(raw), &info, NonZeroU32::new(100).unwrap())
        .unwrap()
}

fn contact(
    tracking_id: i32,
    slot: u32,
    state: ContactState,
    x_mm: Option<Millimeters>,
    y_mm: Option<Millimeters>,
) -> Contact {
    Contact {
        tracking_id,
        slot,
        x_mm,
        y_mm,
        pressure: None,
        major_mm: None,
        minor_mm: None,
        orientation: None,
        state,
    }
}

fn frame(
    sequence: u64,
    usec: u64,
    discontinuity: bool,
    contacts: Vec<Contact>,
    physical_buttons: PhysicalButtons,
    diagnostics: Vec<Diagnostic>,
) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(usec * 1000),
        sequence,
        discontinuity,
        contacts,
        physical_buttons,
        diagnostics,
    }
}

fn down() -> OutputEvent {
    OutputEvent::ButtonDown(MouseButton::Left)
}

fn up() -> OutputEvent {
    OutputEvent::ButtonUp(MouseButton::Left)
}

fn arbiter_cfg() -> ArbiterConfig {
    ArbiterConfig::new(
        Millimeters::try_new(1.0).unwrap(),
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
    )
    .unwrap()
    .with_tap(
        TapConfig::new(
            true,
            true,
            true,
            std::time::Duration::from_millis(500),
            Millimeters::try_new(1.0).unwrap(),
            std::time::Duration::from_millis(400),
        )
        .unwrap(),
    )
}

/// Replays a fixture through the decoder, returning the committed frames.
fn replay_frames(name: &str) -> Vec<ContactFrame> {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(File::open(fixture_path(name)).unwrap(), &mut decoder)
        .expect("fixture must replay cleanly");
    decoder.into_sink().take_frames()
}

/// Builds the synthetic equivalents of the `m8_tap` fixture frames (same
/// sequence, timestamp, contacts, and button state as the decoder output).
fn synthetic_m8_tap_frames() -> Vec<ContactFrame> {
    vec![
        frame(
            1,
            1000,
            false,
            vec![contact(
                10,
                0,
                ContactState::Began,
                Some(mm(500)),
                Some(mm(400)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            2,
            1100,
            false,
            vec![contact(
                10,
                0,
                ContactState::Active,
                Some(mm(510)),
                Some(mm(405)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            3,
            1200,
            false,
            vec![contact(
                10,
                0,
                ContactState::Ended,
                Some(mm(510)),
                Some(mm(405)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
    ]
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

/// The M8 arbiter is a pure function of the frame stream: replay-derived
/// frames and hand-built synthetic frames with identical content must yield
/// identical decisions, proving the tap policy takes the same path for both.
#[test]
fn replay_and_synthetic_tap_frames_produce_identical_decisions() {
    let replay_frames = replay_frames("m8_tap");
    assert_eq!(replay_frames.len(), 3, "m8_tap: expected 3 frames");
    let synthetic_frames = synthetic_m8_tap_frames();
    assert_eq!(
        replay_frames, synthetic_frames,
        "m8_tap: frame content mismatch"
    );

    let mut via_replay = Arbiter::new(arbiter_cfg());
    let mut via_synthetic = Arbiter::new(arbiter_cfg());
    let d_replay = run_all(&mut via_replay, &replay_frames);
    let d_synthetic = run_all(&mut via_synthetic, &synthetic_frames);
    assert_eq!(d_replay, d_synthetic, "m8_tap: decisions must match");
}

/// The `m8_tap` fixture is a quick, small one-finger contact: a qualifying
/// tap emits a deferred `ButtonDown(Left)` at the release frame and opens the
/// follow-up window. The matching up is committed only when that window
/// expires without a follow-up contact.
#[test]
fn replayed_tap_fixture_emits_one_click_pair() {
    let frames = replay_frames("m8_tap");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);

    // Begin and Active frames produce no output.
    assert!(decisions[0].events.is_empty());
    assert!(decisions[1].events.is_empty());
    assert_eq!(
        decisions[0].tap_drag_phase_after,
        TapDragPhase::FirstTapCandidate
    );
    assert_eq!(
        decisions[1].tap_drag_phase_after,
        TapDragPhase::FirstTapCandidate
    );
    // The release frame emits only the deferred press.
    assert_eq!(decisions[2].events, vec![down()]);
    assert_eq!(
        decisions[2].tap_drag_phase_after,
        TapDragPhase::FollowUpWindow
    );
    assert_eq!(decisions[2].lifecycle_after, Lifecycle::Finished);
    assert_eq!(buttons(&decisions), vec![down()]);
    assert_eq!(
        decisions[2].transitions,
        vec![LifecycleTransition::Finish { tracking_id: 10 }]
    );
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);

    let release_ts = frames[2].monotonic_timestamp;
    let timeout = release_ts.saturating_add(std::time::Duration::from_millis(401));
    let timeout_decision = arbiter.tick(timeout).unwrap();
    assert_eq!(timeout_decision.events, vec![up()]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
}
