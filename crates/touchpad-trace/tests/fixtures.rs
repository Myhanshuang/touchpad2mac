//! Fixture verification: every hand-written JSON Lines fixture in
//! `tests/fixtures/` must read cleanly through the public reader API with the
//! expected header and event content. These fixtures are also the raw-input
//! corpus the M3 decoder will replay.

use std::fs::File;
use std::path::{Path, PathBuf};

use touchpad_trace::{TraceClock, TraceEvent, TraceReader, TraceTime};

/// Every fixture and its expected shape. `name` is the file stem; the other
/// fields are the header invariants asserted for that fixture.
struct FixtureCase {
    name: &'static str,
    expected_events: u64,
    /// Presence of `SYN_DROPPED` (type 0, code 3).
    has_syn_dropped: bool,
    /// Presence of a `BTN_LEFT` key event (type 1, code 272).
    has_button: bool,
    /// Presence of a tracking-id end (`ABS_MT_TRACKING_ID == -1`).
    has_tracking_end: bool,
    /// Whether the X axis (ABS code 53, `AxisId 53` under the Linux layer's
    /// axis-id convention) carries a reported resolution.
    x_has_resolution: bool,
}

const CASES: &[FixtureCase] = &[
    FixtureCase {
        name: "single_contact",
        expected_events: 10,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "multi_slot",
        expected_events: 20,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "buttons",
        expected_events: 11,
        has_syn_dropped: false,
        has_button: true,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "missing_resolution",
        expected_events: 7,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: false,
    },
    FixtureCase {
        name: "dropped_recovery",
        expected_events: 12,
        has_syn_dropped: true,
        has_button: false,
        has_tracking_end: false,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "m7_motion",
        expected_events: 20,
        has_syn_dropped: false,
        has_button: true,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "m8_tap",
        expected_events: 10,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "m9_scroll",
        expected_events: 40,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "m9_secondary_tap",
        expected_events: 16,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
    FixtureCase {
        name: "m11_fidelity",
        expected_events: 60,
        has_syn_dropped: false,
        has_button: false,
        has_tracking_end: true,
        x_has_resolution: true,
    },
];

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jsonl"))
}

/// Reads the header and every event of a fixture file, enforcing the
/// reader's full contract (header first/once, time non-decreasing, field
/// ranges).
fn read_fixture(name: &str) -> (touchpad_trace::TraceHeader, Vec<TraceEvent>) {
    let mut reader = TraceReader::new(File::open(fixture_path(name)).unwrap());
    let header = reader.read_header().unwrap();
    let events = reader.events().map(Result::unwrap).collect();
    (header, events)
}

#[test]
fn all_fixture_files_exist() {
    for case in CASES {
        assert!(
            fixture_path(case.name).is_file(),
            "missing fixture {}",
            case.name
        );
    }
}

#[test]
fn every_fixture_reads_with_valid_header_and_monotonic_times() {
    for case in CASES {
        let (header, events) = read_fixture(case.name);

        // Schema/clock contract.
        assert_eq!(header.schema_version, 1, "{}: schema version", case.name);
        assert_eq!(header.clock, TraceClock::Monotonic, "{}: clock", case.name);

        // Device descriptor: identity, Type-B MT, slot count, X/Y axes.
        assert!(
            header.device.name.contains("fixture"),
            "{}: device name {:?}",
            case.name,
            header.device.name
        );
        assert!(header.device.supports_type_b_mt, "{}: type-b", case.name);
        assert_eq!(
            header.device.slot_count,
            Some(10),
            "{}: slot count",
            case.name
        );
        assert_eq!(header.device.axes.len(), 2, "{}: axis count", case.name);

        let x = header
            .device
            .axes
            .get(&touchpad_core::AxisId::new(53))
            .expect("X axis present (ABS_MT_POSITION_X == 53)");
        assert_eq!((x.min, x.max), (0, 1000), "{}: X range", case.name);
        if case.x_has_resolution {
            assert_eq!(
                x.resolution,
                Some(std::num::NonZeroU32::new(100).unwrap()),
                "{}: X resolution",
                case.name
            );
        } else {
            assert_eq!(x.resolution, None, "{}: X resolution absent", case.name);
        }

        // Count and content.
        assert_eq!(
            events.len() as u64,
            case.expected_events,
            "{}: event count",
            case.name
        );
        assert_eq!(
            events.iter().any(|e| e.event_type == 0 && e.code == 3),
            case.has_syn_dropped,
            "{}: SYN_DROPPED presence",
            case.name
        );
        assert_eq!(
            events.iter().any(|e| e.event_type == 1 && e.code == 272),
            case.has_button,
            "{}: button presence",
            case.name
        );
        assert_eq!(
            events
                .iter()
                .any(|e| e.event_type == 3 && e.code == 57 && e.value == -1),
            case.has_tracking_end,
            "{}: tracking-id end presence",
            case.name
        );

        // The reader already enforces non-decreasing times; double-check by
        // collecting them.
        let times: Vec<TraceTime> = events.iter().map(TraceEvent::time).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "{}: event times non-decreasing", case.name);
    }
}

#[test]
fn missing_resolution_fixture_has_no_fake_resolution() {
    let (header, _) = read_fixture("missing_resolution");
    // A device without reported resolution must stay unnormalized: no axis
    // may pretend to carry one, and the header must validate cleanly as a
    // descriptor (range checks only).
    for (axis, info) in &header.device.axes {
        assert_eq!(
            info.resolution, None,
            "axis {axis:?} must not fake a resolution"
        );
    }
    assert!(header.device.validate().is_empty());
}

#[test]
fn fixture_headers_describe_the_same_device_shape() {
    // All fixtures share the same device geometry; verify the round-trip
    // fixture is structurally identical to the others.
    let (a, _) = read_fixture("single_contact");
    let (b, _) = read_fixture("multi_slot");
    assert_eq!(a.device.axes, b.device.axes);
    assert_eq!(a.device.slot_count, b.device.slot_count);
    let (dropped, _) = read_fixture("dropped_recovery");
    assert_eq!(dropped.device.axes, a.device.axes);
}

/// Splits the raw events of a fixture into frames at `SYN_REPORT` boundaries
/// (`EV_SYN == 0`, `SYN_REPORT == 0`), the same boundary the M3 decoder
/// commits frames at. Every frame carries its closing `SYN_REPORT`.
fn split_frames(events: &[TraceEvent]) -> Vec<Vec<TraceEvent>> {
    let mut frames: Vec<Vec<TraceEvent>> = Vec::new();
    let mut current: Vec<TraceEvent> = Vec::new();
    for event in events {
        current.push(event.clone());
        if event.event_type == 0 && event.code == 0 {
            frames.push(std::mem::take(&mut current));
        }
    }
    assert!(
        current.is_empty(),
        "trace must end at a SYN_REPORT boundary"
    );
    frames
}

/// The last raw X/Y position reported within one frame (`ABS_MT_POSITION_X ==
/// 53`, `ABS_MT_POSITION_Y == 54`), or `None` when the frame reports none.
fn last_reported_position(frame: &[TraceEvent]) -> (Option<i32>, Option<i32>) {
    let mut x = None;
    let mut y = None;
    for event in frame {
        match (event.event_type, event.code) {
            (3, 53) => x = Some(event.value),
            (3, 54) => y = Some(event.value),
            _ => {}
        }
    }
    (x, y)
}

/// The `m11_fidelity` fixture is the deterministic raw trace for the M11
/// trace/replay coverage (Part 3B). This test locks the raw frame structure —
/// frame count, per-frame timestamps, and per-frame reported positions — so
/// any accidental fixture drift is caught at the trace level before the
/// direct-vs-replay decision tests consume it.
///
/// Both axes are `100 units/mm` with `min == 0`, so every reported position
/// normalizes deterministically to `raw / 100` mm (e.g. X 300 -> 3.00 mm, X
/// 575 -> 5.75 mm); every raw value is a multiple of 25 (a quarter
/// millimeter), so each normalized value is exactly representable in `f32`.
///
/// The scene boundaries the frame stream is designed to exercise:
///
/// * frames 1-5: candidate accumulation, then the **first commit** — the full
///   1.00 mm candidate displacement (X 300 -> 400) reaches the fidelity stage
///   as its first call at frame 5;
/// * frames 6-8: **low-speed** continuation (+0.25 mm per 8 ms frame);
/// * frames 9-10: **duplicate frame timestamp** (both 65,000 usec);
/// * frame 11: **diagonal** motion (+0.50 mm on both axes);
/// * frames 12-14: sub-dead-zone hold (+0.05 X), **reversal** (-0.05 X), hold
///   (+0.05 Y);
/// * frames 15-16: **high-speed** movement (+1.50 mm per 8 ms frame);
/// * frame 17: **exactly-long-gap** frame (`dt == 150,000 usec` from frame
///   16, the inclusive M11 `long_gap`);
/// * frame 18: **over-long-gap** frame (`dt == 237,000 usec` from frame 17);
/// * frames 19-21: post-gap motion then the **clean end** (tracking id -1);
/// * frames 22-25: a **fresh new interaction** (tracking id 11) with its own
///   commit and clean end.
#[test]
fn m11_fidelity_fixture_frame_structure_is_stable() {
    let (_, events) = read_fixture("m11_fidelity");
    let frames = split_frames(&events);
    assert_eq!(frames.len(), 25, "expected exactly 25 decoded frames");

    // Expected per-frame (1-based frame number, frame timestamp usec, last
    // reported raw X, last reported raw Y). `None` means the frame carries no
    // update for that axis (at decode time the axis inherits the previous
    // committed position); a frame that reports neither axis is `(None, None)`.
    const EXPECTED: &[(u64, u32, Option<i32>, Option<i32>)] = &[
        (1, 1_000, Some(300), Some(200)),
        (2, 9_000, Some(325), None),
        (3, 17_000, Some(350), None),
        (4, 25_000, Some(375), None),
        (5, 33_000, Some(400), None), // first commit
        (6, 41_000, Some(425), None),
        (7, 49_000, Some(450), None),
        (8, 57_000, Some(475), None),
        (9, 65_000, Some(500), None),
        (10, 65_000, Some(525), None),       // duplicate timestamp
        (11, 73_000, Some(575), Some(250)),  // diagonal
        (12, 81_000, Some(580), None),       // +0.05 mm hold
        (13, 89_000, Some(575), None),       // reversal
        (14, 97_000, None, Some(255)),       // +0.05 mm hold
        (15, 105_000, Some(725), None),      // high speed
        (16, 113_000, Some(875), None),      // high speed
        (17, 263_000, Some(885), None),      // exactly-long-gap
        (18, 500_000, Some(895), Some(260)), // over-long-gap
        (19, 508_000, Some(905), None),
        (20, 516_000, Some(915), None),
        (21, 524_000, Some(920), Some(265)), // clean end
        (22, 600_000, Some(300), Some(500)), // fresh interaction begins
        (23, 608_000, Some(400), None),      // fresh commit
        (24, 616_000, Some(425), None),
        (25, 624_000, None, None), // fresh clean end, no position update
    ];

    for (index, (number, usec, x, y)) in EXPECTED.iter().enumerate() {
        let frame = &frames[index];
        assert_eq!(
            frame.last().unwrap().time(),
            TraceTime {
                sec: 0,
                usec: *usec
            },
            "frame {number}: timestamp"
        );
        assert_eq!(
            last_reported_position(frame),
            (*x, *y),
            "frame {number}: reported positions"
        );
    }

    // Contact lifecycle markers at the raw level: the first contact (tracking
    // id 10) begins in frame 1 and ends cleanly in frame 21; the fresh
    // interaction (tracking id 11) begins in frame 22 and ends cleanly in
    // frame 25.
    fn tracking_ids(frame: &[TraceEvent]) -> Vec<i32> {
        frame
            .iter()
            .filter(|e| e.event_type == 3 && e.code == 57)
            .map(|e| e.value)
            .collect()
    }
    assert_eq!(tracking_ids(&frames[0]), vec![10]);
    assert_eq!(tracking_ids(&frames[20]), vec![-1]);
    assert_eq!(tracking_ids(&frames[21]), vec![11]);
    assert_eq!(tracking_ids(&frames[24]), vec![-1]);

    // The long-gap boundary semantics are built into the timestamps: frame 17
    // is exactly `long_gap` (150,000 usec) after frame 16, and frame 18 is
    // beyond it.
    let dt_17 = TraceTime {
        sec: 0,
        usec: 263_000,
    }
    .to_monotonic()
    .unwrap()
    .duration_since(
        TraceTime {
            sec: 0,
            usec: 113_000,
        }
        .to_monotonic()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(dt_17, std::time::Duration::from_micros(150_000));
    let dt_18 = TraceTime {
        sec: 0,
        usec: 500_000,
    }
    .to_monotonic()
    .unwrap()
    .duration_since(
        TraceTime {
            sec: 0,
            usec: 263_000,
        }
        .to_monotonic()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(dt_18, std::time::Duration::from_micros(237_000));
    assert!(
        dt_18 > std::time::Duration::from_micros(150_000),
        "frame 18 must be over the M11 long gap"
    );
}
