//! M11 integration (Part 3B): the `m11_fidelity` trace replayed through the
//! real Type-B decoder and hand-built synthetic frames produce **identical**
//! M11 Arbiter decisions (M11_TASK.md §12).
//!
//! Replay drives the exact M3 Type-B decoder used by live input (there is no
//! second decoder), producing `ContactFrame`s; synthetic frames are built by
//! hand with identical content. Both streams are fed into separate
//! `Arbiter::new(M11Profile::new().unwrap().arbiter_config())` instances, and
//! the per-frame [`FrameDecision`]s must be equal — proving the M11 Arbiter
//! is a pure, deterministic function of the normalized frame stream
//! regardless of how the frames were produced.
//!
//! Beyond the equality proof, the semantic tests assert that the fixture
//! genuinely exercises every required M11 timing/motion case (M11_TASK.md
//! §7/§9): the first commit preserves the full accumulated displacement at
//! `min_gain`, low-speed motion stays on the min-gain scale, a
//! duplicate-timestamp frame folds displacement without flushing or
//! fabricating velocity, the next positive-`dt` velocity sample includes that
//! displacement, reversal/diagonal motion obeys the signed radial dead zone,
//! exactly-`long_gap` and over-`long_gap` frames re-anchor with **no**
//! gap-crossing pointer output, the clean end resets all state, and the fresh
//! second interaction starts with no stale velocity/remainder.
//!
//! All tests are offline: no hardware is opened, no portal/libei session is
//! created, no desktop input is emitted, and nothing sleeps.

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use touchpad_core::{
    raw_axis_position_to_mm_with_resolution, Arbiter, ArbiterConfig, AxisInfo, Contact,
    ContactFrame, ContactState, Lifecycle, LifecycleTransition, LogicalPixels, M11Profile,
    Millimeters, Monotonic, OutputEvent, PhysicalButtons, RawAxis,
};
use touchpad_linux::{RecordingFrameSink, TypeBDecoder};
use touchpad_trace::ReplayDriver;

/// Frame indexes (0-based, sequence = index + 1) of the fixture's semantic
/// waypoints, so every assertion reads against a named stage.
const FIRST_COMMIT: usize = 4; // seq 5: candidate crosses the 1 mm threshold
const LOW_SPEED: std::ops::Range<usize> = 5..9; // seq 6-9: 0.25 mm per 8 ms
const DUPLICATE: usize = 9; // seq 10: same timestamp as seq 9
const DIAGONAL: usize = 10; // seq 11: (0.5, 0.5) mm + duplicate (0.25, 0)
const REVERSAL_HOLDS: [usize; 3] = [11, 12, 13]; // seq 12-14: +0.05/-0.05/0.05
const HIGH_SPEED: [usize; 2] = [14, 15]; // seq 15-16: 1.5 mm per 8 ms
const EXACT_GAP: usize = 16; // seq 17: dt == 150 ms == long_gap
const OVER_GAP: usize = 17; // seq 18: dt == 237 ms > long_gap
const AFTER_REANCHOR: std::ops::Range<usize> = 18..20; // seq 19-20
const CLEAN_END: usize = 20; // seq 21: Ended, interaction 1
const SECOND_BEGIN: usize = 21; // seq 22: Began, interaction 2
const SECOND_COMMIT: usize = 22; // seq 23: interaction 2's first commit
const SECOND_END: usize = 24; // seq 25: Ended, interaction 2

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../touchpad-trace/tests/fixtures")
        .join(format!("{name}.jsonl"))
}

/// Raw count -> millimeters exactly as the decoder normalizes it: the
/// fixture header declares axis 53/54 `min 0 max 1000 resolution 100`, so
/// `mm = raw / 100` (both paths use
/// [`raw_axis_position_to_mm_with_resolution`]).
fn mm(raw: i32) -> Millimeters {
    let info = AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100));
    raw_axis_position_to_mm_with_resolution(RawAxis::new(raw), &info, NonZeroU32::new(100).unwrap())
        .unwrap()
}

fn contact(tracking_id: i32, state: ContactState, x_raw: i32, y_raw: i32) -> Contact {
    Contact {
        tracking_id,
        slot: 0,
        x_mm: Some(mm(x_raw)),
        y_mm: Some(mm(y_raw)),
        pressure: None,
        major_mm: None,
        minor_mm: None,
        orientation: None,
        state,
    }
}

/// A `SYN_REPORT` frame at `usec` microseconds (the fixture's timestamp
/// resolution) for the single Type-B slot, with no physical buttons and no
/// diagnostics — matching the decoder's output for this clean fixture.
fn frame(sequence: u64, usec: u64, contact: Contact) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(usec * 1000),
        sequence,
        discontinuity: false,
        contacts: vec![contact],
        physical_buttons: PhysicalButtons::NONE,
        diagnostics: vec![],
    }
}

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: LogicalPixels::try_new(dx).unwrap(),
        dy: LogicalPixels::try_new(dy).unwrap(),
    }
}

/// The validated `m11-fidelity-v1` arbiter configuration (M11_TASK.md §5).
fn m11_cfg() -> ArbiterConfig {
    M11Profile::new().unwrap().arbiter_config()
}

/// Replays the `m11_fidelity` fixture through the real Type-B decoder,
/// returning the committed `ContactFrame`s in order.
fn replay_frames() -> Vec<ContactFrame> {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(
        File::open(fixture_path("m11_fidelity")).unwrap(),
        &mut decoder,
    )
    .expect("fixture must replay cleanly");
    decoder.into_sink().take_frames()
}

/// The hand-built synthetic equivalents of the `m11_fidelity` decoder
/// output: identical sequence, timestamp, contact, and button state for all
/// 25 frames.
fn synthetic_frames() -> Vec<ContactFrame> {
    // Interaction 1 (tracking 10): slow approach, first commit, low-speed
    // run, a duplicate-timestamp frame, diagonal/high-speed motion, a
    // reversal, two long-gap re-anchors, and a clean end.
    let mut out = vec![
        frame(1, 1_000, contact(10, ContactState::Began, 300, 200)),
        frame(2, 9_000, contact(10, ContactState::Active, 325, 200)),
        frame(3, 17_000, contact(10, ContactState::Active, 350, 200)),
        frame(4, 25_000, contact(10, ContactState::Active, 375, 200)),
        frame(5, 33_000, contact(10, ContactState::Active, 400, 200)),
        frame(6, 41_000, contact(10, ContactState::Active, 425, 200)),
        frame(7, 49_000, contact(10, ContactState::Active, 450, 200)),
        frame(8, 57_000, contact(10, ContactState::Active, 475, 200)),
        frame(9, 65_000, contact(10, ContactState::Active, 500, 200)),
        frame(10, 65_000, contact(10, ContactState::Active, 525, 200)),
        frame(11, 73_000, contact(10, ContactState::Active, 575, 250)),
        frame(12, 81_000, contact(10, ContactState::Active, 580, 250)),
        frame(13, 89_000, contact(10, ContactState::Active, 575, 250)),
        frame(14, 97_000, contact(10, ContactState::Active, 575, 255)),
        frame(15, 105_000, contact(10, ContactState::Active, 725, 255)),
        frame(16, 113_000, contact(10, ContactState::Active, 875, 255)),
        frame(17, 263_000, contact(10, ContactState::Active, 885, 255)),
        frame(18, 500_000, contact(10, ContactState::Active, 895, 260)),
        frame(19, 508_000, contact(10, ContactState::Active, 905, 260)),
        frame(20, 516_000, contact(10, ContactState::Active, 915, 260)),
        frame(21, 524_000, contact(10, ContactState::Ended, 920, 265)),
    ];
    // Interaction 2 (tracking 11): a fresh contact after a 76 ms gap commits
    // and ends cleanly.
    out.extend([
        frame(22, 600_000, contact(11, ContactState::Began, 300, 500)),
        frame(23, 608_000, contact(11, ContactState::Active, 400, 500)),
        frame(24, 616_000, contact(11, ContactState::Active, 425, 500)),
        frame(25, 624_000, contact(11, ContactState::Ended, 425, 500)),
    ]);
    out
}

fn run_all(arbiter: &mut Arbiter, frames: &[ContactFrame]) -> Vec<touchpad_core::FrameDecision> {
    frames
        .iter()
        .map(|frame| arbiter.frame(frame).expect("frame must be accepted"))
        .collect()
}

/// The ordered `PointerMove` deltas across a slice of decisions.
fn moves(decisions: &[touchpad_core::FrameDecision]) -> Vec<(f32, f32)> {
    decisions
        .iter()
        .flat_map(|d| d.events.iter())
        .filter_map(|e| match e {
            OutputEvent::PointerMove { dx, dy } => Some((dx.as_px(), dy.as_px())),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Direct vs. replay equality (M11_TASK.md §12)
// ---------------------------------------------------------------------------

/// The M11 Arbiter is a pure function of the frame stream: replay-derived
/// frames and hand-built synthetic frames with identical content must yield
/// identical decisions, one by one, for the whole `m11_fidelity` fixture.
#[test]
fn replay_and_synthetic_frames_produce_identical_decisions() {
    let replay_frames = replay_frames();
    assert_eq!(replay_frames.len(), 25, "fixture must decode to 25 frames");
    let synthetic_frames = synthetic_frames();
    assert_eq!(
        replay_frames, synthetic_frames,
        "m11_fidelity: frame content mismatch"
    );

    let mut via_replay = Arbiter::new(m11_cfg());
    let mut via_synthetic = Arbiter::new(m11_cfg());
    let d_replay = run_all(&mut via_replay, &replay_frames);
    let d_synthetic = run_all(&mut via_synthetic, &synthetic_frames);
    assert_eq!(d_replay, d_synthetic, "decisions must match frame by frame");
}

// ---------------------------------------------------------------------------
// Semantic coverage: the fixture genuinely exercises the M11 timing/motion
// cases (M11_TASK.md §7/§9)
// ---------------------------------------------------------------------------

/// First commit: the candidate period emits nothing, then the whole 1.00 mm
/// accumulated displacement commits exactly once at the initial filtered
/// velocity 0 — hence `min_gain` — as 1.00 mm × 10 px/mm = 10 px with a clean
/// remainder. Low-speed motion (0.25 mm per 8 ms = 31.25 mm/s, below
/// `gain_x0`) stays on the min-gain scale: 2.5 px per frame through the exact
/// per-axis remainder, emitting 2, 3, 2, 3.
#[test]
fn fixture_first_commit_preserves_full_motion_and_low_speed_stays_at_min_gain() {
    let frames = replay_frames();
    let decisions = run_all(&mut Arbiter::new(m11_cfg()), &frames);

    // Candidate period: no output at all before the threshold is crossed.
    for d in decisions.iter().take(FIRST_COMMIT) {
        assert!(d.events.is_empty(), "candidate frames must emit nothing");
        assert_eq!(d.lifecycle_after, Lifecycle::Candidate);
    }
    assert_eq!(
        decisions[0].transitions,
        vec![LifecycleTransition::Begin { tracking_id: 10 }]
    );

    // The commit frame carries the full 1.0 mm from the candidate anchor,
    // exactly once, at min gain: 10 px, remainder clean.
    assert_eq!(decisions[FIRST_COMMIT].events, vec![move_event(10.0, 0.0)]);
    assert_eq!(
        decisions[FIRST_COMMIT].transitions,
        vec![LifecycleTransition::Commit { tracking_id: 10 }]
    );
    assert_eq!(
        decisions[FIRST_COMMIT].lifecycle_after,
        Lifecycle::Committed
    );

    // Low-speed frames: each 0.25 mm at min gain scales to exactly 2.5 px,
    // so the per-axis remainder alternates 0.5/0.0 and the emitted stream is
    // 2, 3, 2, 3 (never a 2.5 px event, never a lost fraction).
    assert_eq!(
        moves(&decisions[LOW_SPEED]),
        vec![(2.0, 0.0), (3.0, 0.0), (2.0, 0.0), (3.0, 0.0)]
    );
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = run_all(&mut arbiter, &frames[..LOW_SPEED.end]);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = run_all(&mut arbiter, &frames[..=LOW_SPEED.start]);
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));
}

/// Duplicate timestamp: the seq 9 and seq 10 frames carry the same monotonic
/// timestamp. The duplicate frame folds its 0.25 mm into `P` (norm 0.25 >=
/// the 0.09 mm radius) but the dead zone is only evaluated after a velocity
/// update, so nothing is emitted and no velocity is fabricated. The
/// displacement then participates exactly once in the next positive-`dt`
/// sample: the seq 11 velocity sample is (0.75, 0.5) mm / 8 ms, emitting
/// (7, 5) px — strictly more than the (5, 5) px a dropped duplicate would
/// yield at min gain.
#[test]
fn fixture_duplicate_timestamp_holds_without_flush_and_feeds_the_next_sample() {
    let frames = replay_frames();
    let decisions = run_all(&mut Arbiter::new(m11_cfg()), &frames);

    assert_eq!(
        frames[DUPLICATE - 1].monotonic_timestamp,
        frames[DUPLICATE].monotonic_timestamp,
        "seq 9 and seq 10 must share a timestamp"
    );
    assert_eq!(frames[DUPLICATE].contacts[0].x_mm, Some(mm(525)));
    // Folded 0.25 mm exceeds the dead-zone radius, yet the duplicate frame
    // neither flushes it nor emits anything.
    assert!(decisions[DUPLICATE].events.is_empty());
    assert!(decisions[DUPLICATE].transitions.is_empty());
    // The next positive-dt frame's velocity sample includes the duplicate
    // displacement (0.75 mm x), producing a (7, 5) px move.
    assert_eq!(decisions[DIAGONAL].events, vec![move_event(7.0, 5.0)]);
}

/// Reversal and diagonal motion: the diagonal (0.5, 0.5) mm step (plus the
/// duplicate's 0.25 mm) emits on both axes at once; the +0.05 mm forward then
/// -0.05 mm backward reversal cancels algebraically inside the signed radial
/// dead zone, and the subsequent 0.05 mm steps stay below the radius — frames
/// 12-14 emit no pointer output while the interaction stays committed.
#[test]
fn fixture_diagonal_and_reversal_motion_obey_the_signed_dead_zone() {
    let frames = replay_frames();
    let decisions = run_all(&mut Arbiter::new(m11_cfg()), &frames);

    // Diagonal: both axes emitted in one move.
    assert_eq!(decisions[DIAGONAL].events, vec![move_event(7.0, 5.0)]);

    // Reversal: seq 12 advances x 5.75 -> 5.80 mm, seq 13 returns to 5.75 mm.
    assert_eq!(frames[REVERSAL_HOLDS[0]].contacts[0].x_mm, Some(mm(580)));
    assert_eq!(frames[REVERSAL_HOLDS[1]].contacts[0].x_mm, Some(mm(575)));
    for i in REVERSAL_HOLDS {
        assert!(
            decisions[i].events.is_empty(),
            "below-radius/reversal frame {i} must emit no pointer output"
        );
        assert_eq!(decisions[i].lifecycle_after, Lifecycle::Committed);
    }
}

/// Long gaps: the seq 16 -> seq 17 gap is exactly `long_gap` (150 ms,
/// inclusive boundary) and the seq 17 -> seq 18 gap is 237 ms (above it).
/// Both gap-crossing displacements are discarded before folding, the stage
/// re-anchors, and **no pointer output and no lifecycle transition** appears
/// on either gap frame. Afterward the stage resumes fresh: the two following
/// 0.1 mm steps emit 1 px each at min gain.
#[test]
fn fixture_exact_and_over_long_gap_reanchors_discard_gap_crossing_motion() {
    let frames = replay_frames();
    let decisions = run_all(&mut Arbiter::new(m11_cfg()), &frames);

    let exact = frames[EXACT_GAP]
        .monotonic_timestamp
        .duration_since(frames[HIGH_SPEED[1]].monotonic_timestamp)
        .unwrap();
    assert_eq!(exact, Duration::from_millis(150), "seq 16 -> 17 gap");
    let over = frames[OVER_GAP]
        .monotonic_timestamp
        .duration_since(frames[EXACT_GAP].monotonic_timestamp)
        .unwrap();
    assert_eq!(over, Duration::from_millis(237), "seq 17 -> 18 gap");

    for i in [EXACT_GAP, OVER_GAP] {
        assert!(
            decisions[i].events.is_empty(),
            "gap frame {i} must emit no pointer output"
        );
        assert!(
            decisions[i].transitions.is_empty(),
            "a long-gap re-anchor is not a lifecycle event (frame {i})"
        );
        assert_eq!(decisions[i].lifecycle_after, Lifecycle::Committed);
    }

    // Post-re-anchor: fresh velocity state, below `gain_x0`, so each 0.1 mm
    // step emits exactly 1 px at min gain.
    assert_eq!(
        moves(&decisions[AFTER_REANCHOR]),
        vec![(1.0, 0.0), (1.0, 0.0)]
    );
}

/// Clean end and a fresh second interaction: the Ended frame (seq 21) carries
/// its final coordinates; its final (0.05, 0.05) mm movement is below the
/// dead-zone radius and is discarded at the interaction reset — no pointer
/// output, then `Finish`, with the remainder cleared. The seq 22-25 second
/// interaction begins with a fresh candidate and its first commit emits
/// exactly 10 px at min gain with a clean remainder: no stale velocity, `P`,
/// or subpixel remainder leaks from interaction 1.
#[test]
fn fixture_clean_end_and_fresh_second_interaction_leave_no_stale_state() {
    let frames = replay_frames();
    let decisions = run_all(&mut Arbiter::new(m11_cfg()), &frames);

    // Clean end of interaction 1: final below-radius movement held, then
    // Finish, then reset.
    assert_eq!(frames[CLEAN_END].contacts[0].state, ContactState::Ended);
    assert!(decisions[CLEAN_END].events.is_empty());
    assert_eq!(
        decisions[CLEAN_END].transitions,
        vec![LifecycleTransition::Finish { tracking_id: 10 }]
    );
    assert_eq!(decisions[CLEAN_END].lifecycle_after, Lifecycle::Finished);

    // The fresh second interaction starts cleanly: a new Begin with no output.
    assert_eq!(
        decisions[SECOND_BEGIN].transitions,
        vec![LifecycleTransition::Begin { tracking_id: 11 }]
    );
    assert!(decisions[SECOND_BEGIN].events.is_empty());
    assert_eq!(
        decisions[SECOND_BEGIN].lifecycle_after,
        Lifecycle::Candidate
    );

    // Interaction 2's first commit is a *fresh* first fidelity call: the
    // whole 1.0 mm at min gain emits exactly 10 px with a clean remainder. A
    // leaked velocity/`P` from interaction 1 (e.g. 7.5 mm/s and 0.05 mm
    // remainders) would scale this to ~10.5 px with a nonzero remainder.
    assert_eq!(decisions[SECOND_COMMIT].events, vec![move_event(10.0, 0.0)]);
    assert_eq!(
        decisions[SECOND_COMMIT].transitions,
        vec![LifecycleTransition::Commit { tracking_id: 11 }]
    );
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = run_all(&mut arbiter, &frames[..=SECOND_COMMIT]);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // The rest of interaction 2 follows the same min-gain pattern and ends
    // cleanly with the remainder reset.
    assert_eq!(
        moves(&decisions[SECOND_COMMIT + 1..SECOND_END]),
        vec![(2.0, 0.0)]
    );
    assert_eq!(
        decisions[SECOND_END].transitions,
        vec![LifecycleTransition::Finish { tracking_id: 11 }]
    );
    assert_eq!(decisions[SECOND_END].lifecycle_after, Lifecycle::Finished);
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = run_all(&mut arbiter, &frames);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);
}
