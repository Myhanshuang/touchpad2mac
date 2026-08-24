//! M7 integration: trace/replay-derived frames and directly synthetic frames
//! exercise the **same** arbiter path.
//!
//! Replay drives the exact M3 Type-B decoder used by live input (there is no
//! second decoder), producing `ContactFrame`s; synthetic frames are built by
//! hand with identical content. Both are fed to the M7 `Arbiter`, and the
//! per-frame decisions must be identical — proving the arbiter is a pure,
//! deterministic function of the normalized frame stream regardless of how
//! the frames were produced.

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use touchpad_core::{
    raw_axis_position_to_mm_with_resolution, Arbiter, ArbiterConfig, AxisInfo, Contact,
    ContactFrame, ContactState, Diagnostic, FrameDecision, Lifecycle, LifecycleTransition,
    LogicalPixels, LogicalPixelsPerMm, Millimeters, Monotonic, MouseButton, OutputEvent,
    PhysicalButtons, RawAxis,
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

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: LogicalPixels::try_new(dx).unwrap(),
        dy: LogicalPixels::try_new(dy).unwrap(),
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
}

/// Replays a fixture through the decoder, returning the committed frames.
fn replay_frames(name: &str) -> Vec<ContactFrame> {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(File::open(fixture_path(name)).unwrap(), &mut decoder)
        .expect("fixture must replay cleanly");
    decoder.into_sink().take_frames()
}

/// Builds the synthetic equivalents of the `m7_motion` fixture frames (same
/// sequence, timestamp, contacts, and button state as the decoder output).
fn synthetic_m7_motion_frames() -> Vec<ContactFrame> {
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
                Some(mm(525)),
                Some(mm(400)),
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
                ContactState::Active,
                Some(mm(550)),
                Some(mm(400)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            4,
            1300,
            false,
            vec![contact(
                10,
                0,
                ContactState::Active,
                Some(mm(625)),
                Some(mm(425)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            5,
            1400,
            false,
            vec![contact(
                10,
                0,
                ContactState::Active,
                Some(mm(650)),
                Some(mm(425)),
            )],
            PhysicalButtons::new(true, false, false),
            vec![],
        ),
        frame(
            6,
            1500,
            false,
            vec![contact(
                10,
                0,
                ContactState::Active,
                Some(mm(675)),
                Some(mm(425)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            7,
            1600,
            false,
            vec![contact(
                10,
                0,
                ContactState::Ended,
                Some(mm(675)),
                Some(mm(425)),
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

fn buttons(decisions: &[FrameDecision]) -> Vec<OutputEvent> {
    decisions
        .iter()
        .flat_map(|d| d.events.iter())
        .filter(|e| matches!(e, OutputEvent::ButtonDown(_) | OutputEvent::ButtonUp(_)))
        .cloned()
        .collect()
}

/// The arbiter is a pure function of the frame stream: replay-derived frames
/// and hand-built synthetic frames with identical content must yield
/// identical decisions, one by one, for every fixture.
#[test]
fn replay_and_synthetic_frames_produce_identical_decisions() {
    for name in ["single_contact", "buttons", "m7_motion"] {
        let replay_frames = replay_frames(name);
        assert!(!replay_frames.is_empty(), "{name}: expected frames");
        let synthetic_frames = match name {
            "m7_motion" => synthetic_m7_motion_frames(),
            _ => replay_frames.clone(), // synthetic == decoded content is the identity proof
        };
        assert_eq!(
            replay_frames, synthetic_frames,
            "{name}: frame content mismatch"
        );

        let mut via_replay = Arbiter::new(arbiter_cfg());
        let mut via_synthetic = Arbiter::new(arbiter_cfg());
        let d_replay = run_all(&mut via_replay, &replay_frames);
        let d_synthetic = run_all(&mut via_synthetic, &synthetic_frames);
        assert_eq!(d_replay, d_synthetic, "{name}: decisions must match");
    }
}

/// The `m7_motion` fixture crosses the 1 mm threshold, commits exactly once,
/// drags while the physical button is held (press precedes movement, final
/// movement precedes release), and finishes cleanly.
#[test]
fn replayed_motion_fixture_commits_once_drags_and_finishes() {
    let frames = replay_frames("m7_motion");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);

    // Accumulated (1.25, 0.25) mm from the anchor committed exactly once as
    // (12, 2) (12.5 px truncates to 12, remainder 0.5 carried); subsequent
    // frames are incremental drag deltas.
    assert_eq!(moves(&decisions), vec![(12.0, 2.0), (3.0, 0.0), (2.0, 0.0)]);
    // Physical click while dragging: down/up exactly once.
    assert_eq!(buttons(&decisions), vec![down(), up()]);
    // Same-frame ordering: frame 5 presses and moves (down then move); frame
    // 6 releases and moves (move then up).
    assert_eq!(decisions[4].events, vec![down(), move_event(3.0, 0.0)]);
    assert_eq!(decisions[5].events, vec![move_event(2.0, 0.0), up()]);
    // Clean finish.
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);
    assert_eq!(
        decisions[6].transitions,
        vec![LifecycleTransition::Finish { tracking_id: 10 }]
    );
}

/// The `single_contact` fixture begins and ends below the 1 mm threshold: the
/// arbiter emits nothing and finishes cleanly.
#[test]
fn replayed_below_threshold_fixture_produces_no_output() {
    let frames = replay_frames("single_contact");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);
    assert!(decisions.iter().all(|d| d.events.is_empty()));
    assert_eq!(decisions[2].lifecycle_after, Lifecycle::Finished);
}

/// The `buttons` fixture presses and releases the physical left button while
/// the contact stays still: exactly one down and one up, nothing else.
#[test]
fn replayed_buttons_fixture_emits_one_click() {
    let frames = replay_frames("buttons");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);
    assert_eq!(buttons(&decisions), vec![down(), up()]);
    assert!(moves(&decisions).is_empty());
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);
}
