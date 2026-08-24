//! M9 integration: trace/replay-derived frames and directly synthetic frames
//! exercise the **same** arbiter path for the two-finger scroll /
//! secondary-tap policy.
//!
//! Replay drives the exact M3 Type-B decoder used by live input (there is no
//! second decoder), producing `ContactFrame`s; synthetic frames are built by
//! hand with identical content. Both are fed to the M9 `Arbiter` (with a
//! validated two-finger configuration), and the per-frame decisions must be
//! identical — proving the two-finger policy is a pure, deterministic
//! function of the normalized frame stream regardless of how the frames were
//! produced.

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use touchpad_core::{
    raw_axis_position_to_mm_with_resolution, Arbiter, ArbiterConfig, AxisInfo, Contact,
    ContactFrame, ContactState, Diagnostic, FrameDecision, LogicalPixels, LogicalPixelsPerMm,
    Millimeters, Monotonic, MouseButton, OutputEvent, PhysicalButtons, RawAxis, TwoFingerConfig,
    TwoFingerPhase,
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

fn px(x: f32) -> LogicalPixels {
    LogicalPixels::try_new(x).unwrap()
}

fn scroll_delta(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::ScrollDelta {
        dx: px(dx),
        dy: px(dy),
    }
}

fn right_down() -> OutputEvent {
    OutputEvent::ButtonDown(MouseButton::Right)
}

fn right_up() -> OutputEvent {
    OutputEvent::ButtonUp(MouseButton::Right)
}

/// M9 test two-finger config: scroll enabled (natural), ppm 10, 0.5 mm scroll
/// commit threshold, secondary tap enabled, buttonpad two-finger physical
/// click enabled, 500 ms tap duration, 2 mm tap movement limit.
fn arbiter_cfg() -> ArbiterConfig {
    ArbiterConfig::new(
        Millimeters::try_new(1.0).unwrap(),
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
    )
    .unwrap()
    .with_two_finger(
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            Millimeters::try_new(0.5).unwrap(),
            true,
            true,
            std::time::Duration::from_millis(500),
            Millimeters::try_new(2.0).unwrap(),
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

/// Builds the synthetic equivalents of the `m9_scroll` fixture frames (same
/// sequence, timestamp, contacts, and button state as the decoder output).
/// Two fingers scroll diagonally in exact 0.25 mm steps; the scroll commits
/// on frame 5 and ends on frame 9.
fn synthetic_m9_scroll_frames() -> Vec<ContactFrame> {
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
            vec![
                contact(10, 0, ContactState::Active, Some(mm(500)), Some(mm(400))),
                contact(11, 1, ContactState::Began, Some(mm(600)), Some(mm(450))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            3,
            1200,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(525)), Some(mm(425))),
                contact(11, 1, ContactState::Active, Some(mm(600)), Some(mm(450))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            4,
            1300,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(525)), Some(mm(425))),
                contact(11, 1, ContactState::Active, Some(mm(625)), Some(mm(475))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            5,
            1400,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(550)), Some(mm(450))),
                contact(11, 1, ContactState::Active, Some(mm(625)), Some(mm(475))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            6,
            1500,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(550)), Some(mm(450))),
                contact(11, 1, ContactState::Active, Some(mm(650)), Some(mm(500))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            7,
            1600,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(575)), Some(mm(475))),
                contact(11, 1, ContactState::Active, Some(mm(650)), Some(mm(500))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            8,
            1700,
            false,
            vec![
                contact(10, 0, ContactState::Active, Some(mm(575)), Some(mm(475))),
                contact(11, 1, ContactState::Active, Some(mm(675)), Some(mm(525))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            9,
            1800,
            false,
            vec![
                contact(10, 0, ContactState::Ended, Some(mm(575)), Some(mm(475))),
                contact(11, 1, ContactState::Active, Some(mm(675)), Some(mm(525))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            10,
            1900,
            false,
            vec![contact(
                11,
                1,
                ContactState::Ended,
                Some(mm(675)),
                Some(mm(525)),
            )],
            PhysicalButtons::NONE,
            vec![],
        ),
    ]
}

/// Builds the synthetic equivalents of the `m9_secondary_tap` fixture frames:
/// a quick two-finger contact that lifts staggered (first one finger, then
/// the other), qualifying as one secondary tap at the first boundary that
/// ends the exactly-two interaction.
fn synthetic_m9_secondary_tap_frames() -> Vec<ContactFrame> {
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
            vec![
                contact(10, 0, ContactState::Active, Some(mm(500)), Some(mm(400))),
                contact(11, 1, ContactState::Began, Some(mm(600)), Some(mm(450))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            3,
            1200,
            false,
            vec![
                contact(10, 0, ContactState::Ended, Some(mm(500)), Some(mm(400))),
                contact(11, 1, ContactState::Active, Some(mm(600)), Some(mm(450))),
            ],
            PhysicalButtons::NONE,
            vec![],
        ),
        frame(
            4,
            1300,
            false,
            vec![contact(
                11,
                1,
                ContactState::Ended,
                Some(mm(600)),
                Some(mm(450)),
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

/// The M9 arbiter is a pure function of the frame stream: replay-derived
/// frames and hand-built synthetic frames with identical content must yield
/// identical decisions, proving the two-finger policy takes the same path for
/// both.
#[test]
fn replay_and_synthetic_frames_produce_identical_decisions() {
    for name in ["m9_scroll", "m9_secondary_tap"] {
        let replay_frames = replay_frames(name);
        assert!(!replay_frames.is_empty(), "{name}: expected frames");
        let synthetic_frames = match name {
            "m9_scroll" => synthetic_m9_scroll_frames(),
            _ => synthetic_m9_secondary_tap_frames(),
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

/// The `m9_scroll` fixture is a diagonal two-finger scroll: the candidate
/// anchors on frame 2, commits on frame 5 with `ScrollBegin` plus the
/// accumulated centroid displacement exactly once, continues with per-frame
/// incremental deltas preserving **both** non-zero axes (diagonal motion is
/// first-class), and ends with exactly one `ScrollEnd` on frame 9.
#[test]
fn replayed_scroll_fixture_emits_diagonal_scroll_lifecycle() {
    let frames = replay_frames("m9_scroll");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);

    // Candidate period (frames 1-4): no pointer, button, or scroll event.
    assert!(decisions[0].events.is_empty());
    assert!(decisions[1].events.is_empty());
    assert!(decisions[2].events.is_empty());
    assert!(decisions[3].events.is_empty());
    assert_eq!(
        decisions[3].two_finger_phase_after,
        TwoFingerPhase::Candidate
    );

    // Frame 5: ScrollBegin + accumulated (3, 3) px exactly once.
    assert_eq!(
        decisions[4].events,
        vec![OutputEvent::ScrollBegin, scroll_delta(3.0, 3.0)]
    );
    assert_eq!(
        decisions[4].two_finger_phase_after,
        TwoFingerPhase::CommittedScroll
    );

    // Frames 6-8: incremental deltas with both axes preserved.
    assert_eq!(decisions[5].events, vec![scroll_delta(2.0, 2.0)]);
    assert_eq!(decisions[6].events, vec![scroll_delta(1.0, 1.0)]);
    assert_eq!(decisions[7].events, vec![scroll_delta(1.0, 1.0)]);

    // Frame 9 (one finger lifts): exactly one ScrollEnd, no secondary tap.
    assert_eq!(decisions[8].events, vec![OutputEvent::ScrollEnd]);
    assert_eq!(
        decisions[8].two_finger_phase_after,
        TwoFingerPhase::Finished
    );

    // Frame 10: the remaining Ended contact produces nothing.
    assert!(decisions[9].events.is_empty());

    // No button output anywhere in the scroll fixture.
    for decision in &decisions {
        assert!(!decision
            .events
            .iter()
            .any(|e| matches!(e, OutputEvent::ButtonDown(_) | OutputEvent::ButtonUp(_))));
    }
    assert_eq!(arbiter.two_finger_phase(), TwoFingerPhase::Finished);
}

/// The `m9_secondary_tap` fixture is a quick, small two-finger contact that
/// lifts staggered: the qualifying secondary tap fires exactly one
/// `ButtonDown(Right), ButtonUp(Right)` pair at the first boundary that ends
/// the exactly-two interaction, and the remaining old Active contact
/// generates no primary pointer/tap output.
#[test]
fn replayed_secondary_tap_fixture_emits_one_right_click_pair() {
    let frames = replay_frames("m9_secondary_tap");
    let mut arbiter = Arbiter::new(arbiter_cfg());
    let decisions = run_all(&mut arbiter, &frames);

    assert_eq!(decisions[0].events, Vec::<OutputEvent>::new());
    assert_eq!(decisions[1].events, Vec::<OutputEvent>::new());
    assert_eq!(
        decisions[1].two_finger_phase_after,
        TwoFingerPhase::Candidate
    );
    // The first boundary that ends the exactly-two interaction fires the
    // right click pair in order, with no interleaved pointer/scroll output.
    assert_eq!(decisions[2].events, vec![right_down(), right_up()]);
    assert_eq!(
        decisions[2].two_finger_phase_after,
        TwoFingerPhase::Finished
    );
    // The remaining old Active contact generates no primary output, and the
    // tap does not fire again.
    assert_eq!(decisions[3].events, Vec::<OutputEvent>::new());
    assert_eq!(arbiter.two_finger_phase(), TwoFingerPhase::Finished);
}
