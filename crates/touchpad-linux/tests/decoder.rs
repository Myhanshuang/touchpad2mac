//! Integration tests for the M3 Type-B decoder: fixture replay through the
//! exact same decoder state machine used by live raw input, and `SYN_DROPPED`
//! resynchronization with a mocked kernel snapshot.
//!
//! The fixtures live in `touchpad-trace`'s test corpus; replaying them here
//! proves the M2 raw-trace boundary drives the M3 decoder end to end, and the
//! parity test proves replay enters the **same** decoder as live input (no
//! second decoder exists).

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use touchpad_core::{
    raw_axis_position_to_mm_with_resolution, AxisInfo, Contact, ContactFrame, ContactState,
    DeviceDescriptor, Diagnostic, DiagnosticCode, DiagnosticLevel, Millimeters, Monotonic,
    PhysicalButtons, RawAxis,
};
use touchpad_linux::{
    DecodeError, KernelStateSnapshot, RawEvent, RecordingFrameSink, ReplayDecodeError,
    ResyncSource, SlotSnapshot, SyncState, TypeBDecoder, EV_SYN, SYN_DROPPED,
};
use touchpad_trace::{
    ReplayDriver, ReplayError, TraceEvent, TraceHeader, TraceReader, TraceWriter,
};

const FIXTURES: &[&str] = &[
    "single_contact",
    "multi_slot",
    "buttons",
    "missing_resolution",
    "dropped_recovery",
];

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../touchpad-trace/tests/fixtures")
        .join(format!("{name}.jsonl"))
}

fn read_fixture(name: &str) -> (TraceHeader, Vec<TraceEvent>) {
    let mut reader = TraceReader::new(File::open(fixture_path(name)).unwrap());
    let header = reader.read_header().unwrap();
    let events = reader.events().map(Result::unwrap).collect();
    (header, events)
}

/// A Type-B device descriptor matching the fixture geometry (slot count 10,
/// X/Y axes at ABS codes 53/54 with resolution 100).
fn type_b_descriptor() -> DeviceDescriptor {
    let mut device = DeviceDescriptor::new("integration test touchpad", 0x1234, 0x5678);
    device.supports_type_b_mt = true;
    device.slot_count = Some(10);
    device.axes.insert(
        touchpad_core::AxisId::new(53),
        AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100)),
    );
    device.axes.insert(
        touchpad_core::AxisId::new(54),
        AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100)),
    );
    device
}

/// The mock snapshot used for the `dropped_recovery` fixture: the device
/// continued with slot 0 tracking id 7 at (110, 110) after the drop.
fn dropped_snapshot() -> KernelStateSnapshot {
    KernelStateSnapshot::new(
        PhysicalButtons::NONE,
        vec![SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(110)),
            position_y: Some(RawAxis::new(110)),
            ..Default::default()
        }],
    )
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

fn diag(level: DiagnosticLevel, code: DiagnosticCode, message: &str, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(level, code, message.to_string(), sequence)
}

#[test]
fn replay_single_contact_fixture_produces_expected_frames() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(
        File::open(fixture_path("single_contact")).unwrap(),
        &mut decoder,
    )
    .unwrap();
    let frames = decoder.into_sink().take_frames();
    assert_eq!(
        frames,
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
                    Some(mm(400))
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
                    Some(mm(520)),
                    Some(mm(405))
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
                    Some(mm(520)),
                    Some(mm(405))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
        ]
    );
}

#[test]
fn replay_multi_slot_fixture_produces_expected_frames() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(
        File::open(fixture_path("multi_slot")).unwrap(),
        &mut decoder,
    )
    .unwrap();
    let frames = decoder.into_sink().take_frames();
    assert_eq!(
        frames,
        vec![
            frame(
                1,
                1000,
                false,
                vec![
                    contact(1, 0, ContactState::Began, Some(mm(100)), Some(mm(200))),
                    contact(2, 1, ContactState::Began, Some(mm(300)), Some(mm(400))),
                ],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                2,
                1100,
                false,
                vec![
                    contact(1, 0, ContactState::Active, Some(mm(110)), Some(mm(200))),
                    contact(2, 1, ContactState::Active, Some(mm(310)), Some(mm(400))),
                ],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                3,
                1200,
                false,
                vec![
                    contact(1, 0, ContactState::Ended, Some(mm(110)), Some(mm(200))),
                    contact(2, 1, ContactState::Active, Some(mm(310)), Some(mm(400))),
                ],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                4,
                1300,
                false,
                vec![contact(
                    2,
                    1,
                    ContactState::Ended,
                    Some(mm(310)),
                    Some(mm(400))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
        ]
    );
}

#[test]
fn replay_buttons_fixture_commits_buttons_atomically_with_frames() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(File::open(fixture_path("buttons")).unwrap(), &mut decoder).unwrap();
    let frames = decoder.into_sink().take_frames();
    assert_eq!(
        frames,
        vec![
            frame(
                1,
                1000,
                false,
                vec![contact(
                    5,
                    0,
                    ContactState::Began,
                    Some(mm(500)),
                    Some(mm(500))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                2,
                1100,
                false,
                vec![contact(
                    5,
                    0,
                    ContactState::Active,
                    Some(mm(500)),
                    Some(mm(500))
                )],
                PhysicalButtons::new(true, false, false),
                vec![],
            ),
            frame(
                3,
                1200,
                false,
                vec![contact(
                    5,
                    0,
                    ContactState::Active,
                    Some(mm(500)),
                    Some(mm(500))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                4,
                1300,
                false,
                vec![contact(
                    5,
                    0,
                    ContactState::Ended,
                    Some(mm(500)),
                    Some(mm(500))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
        ]
    );
}

#[test]
fn replay_missing_resolution_fixture_keeps_coordinates_unnormalized() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(
        File::open(fixture_path("missing_resolution")).unwrap(),
        &mut decoder,
    )
    .unwrap();
    let frames = decoder.into_sink().take_frames();
    assert_eq!(frames.len(), 2);
    // The contact is published with both raw axes reported, but its
    // coordinates stay unnormalized (never a fake millimeter) with one
    // MissingAxisResolution diagnostic per unresolvable axis.
    assert_eq!(
        frames[0].contacts,
        vec![contact(3, 0, ContactState::Began, None, None)]
    );
    let missing = frames[0]
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::MissingAxisResolution)
        .count();
    assert_eq!(missing, 2);
    assert!(frames[0]
        .diagnostics
        .iter()
        .all(|d| d.frame_sequence == Some(1)));
    assert_eq!(
        frames[1].contacts,
        vec![contact(3, 0, ContactState::Ended, None, None)]
    );
}

#[test]
fn replay_dropped_recovery_fixture_publishes_discontinuity_frame() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    decoder.set_resync_source(Box::new(OkResync(dropped_snapshot())));
    ReplayDriver::replay(
        File::open(fixture_path("dropped_recovery")).unwrap(),
        &mut decoder,
    )
    .unwrap();
    let frames = decoder.into_sink().take_frames();
    assert_eq!(
        frames,
        vec![
            frame(
                1,
                1000,
                false,
                vec![contact(
                    7,
                    0,
                    ContactState::Began,
                    Some(mm(100)),
                    Some(mm(100))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
            frame(
                2,
                1100,
                true,
                vec![contact(
                    7,
                    0,
                    ContactState::Began,
                    Some(mm(110)),
                    Some(mm(110))
                )],
                PhysicalButtons::NONE,
                vec![diag(
                    DiagnosticLevel::Info,
                    DiagnosticCode::DecodeRecovered,
                    "input stream lost continuity (SYN_DROPPED) and was resynchronized at frame 2",
                    2,
                )],
            ),
            frame(
                3,
                1200,
                false,
                vec![contact(
                    7,
                    0,
                    ContactState::Active,
                    Some(mm(120)),
                    Some(mm(120))
                )],
                PhysicalButtons::NONE,
                vec![],
            ),
        ]
    );
}

#[test]
fn fixture_replay_and_direct_feed_share_the_same_decoder() {
    // The M3 acceptance invariant: fixture replay must enter the exact same
    // decoder state machine as live raw input. Replaying through
    // `ReplayDriver` (the `ReplaySink` path) and feeding the same raw events
    // directly (the live path) must produce identical frames for every
    // fixture, including the `SYN_DROPPED` one.
    for name in FIXTURES {
        let (header, events) = read_fixture(name);

        // Path A: offline replay through the trace boundary.
        let mut replay_decoder = TypeBDecoder::new(RecordingFrameSink::new());
        if *name == "dropped_recovery" {
            replay_decoder.set_resync_source(Box::new(OkResync(dropped_snapshot())));
        }
        ReplayDriver::replay(File::open(fixture_path(name)).unwrap(), &mut replay_decoder).unwrap();
        let replay_frames = replay_decoder.into_sink().take_frames();

        // Path B: direct feed of raw events, configured with the same
        // descriptor the trace header carries (what live input will do).
        let mut live_decoder = TypeBDecoder::new(RecordingFrameSink::new());
        if *name == "dropped_recovery" {
            live_decoder.set_resync_source(Box::new(OkResync(dropped_snapshot())));
        }
        live_decoder.configure(header.device.clone()).unwrap();
        for event in &events {
            live_decoder
                .feed(RawEvent::from_trace_event(event).unwrap())
                .unwrap();
        }
        let live_frames = live_decoder.into_sink().take_frames();

        assert!(!replay_frames.is_empty(), "{name}: replay produced frames");
        assert_eq!(replay_frames, live_frames, "{name}: replay == live feed");
    }
}

#[test]
fn resync_failure_during_fixture_replay_is_fatal_and_never_emits_trusted_frames() {
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    decoder.set_resync_source(Box::new(FailingResync("injected snapshot failure")));
    let err = ReplayDriver::replay(
        File::open(fixture_path("dropped_recovery")).unwrap(),
        &mut decoder,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ReplayError::Sink(ReplayDecodeError::Decode(DecodeError::ResyncFailed(_)))
    ));
    // Only the pre-drop frame exists; the decoder degraded at the boundary
    // and never produced a trusted frame after the failure.
    let frames = decoder.into_sink().take_frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].sequence, 1);
    assert!(!frames[0].discontinuity);
}

#[test]
fn replay_ending_after_syn_dropped_fails_with_unresolved_sync_loss() {
    // A trace that ends after SYN_DROPPED but before the recovery SYN_REPORT
    // must fail replay: synchronization was never restored, and the decoder
    // must not report clean completion or emit a frame (M3 review R5).
    let device = type_b_descriptor();
    let header = TraceHeader::new(device.clone());
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &header).unwrap();
        writer
            .write_event(&TraceEvent::new(0, 1000, EV_SYN, SYN_DROPPED, 0))
            .unwrap();
        writer.finish().unwrap();
    }
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    let err = ReplayDriver::replay(std::io::Cursor::new(buffer), &mut decoder).unwrap_err();
    assert!(matches!(
        err,
        ReplayError::Sink(ReplayDecodeError::UnresolvedSynchronizationLoss(
            SyncState::DroppedAwaitingBoundary
        ))
    ));
    assert!(
        decoder.into_sink().frames().is_empty(),
        "no frame may be emitted for an unresolved trace"
    );
}

#[test]
fn replay_ending_between_frames_finishes_cleanly() {
    // Ordinary traces end with the decoder in Normal state (between frames):
    // finish must succeed — the distinction from unresolved sync loss is
    // deliberate (M3 review R5).
    let device = type_b_descriptor();
    let header = TraceHeader::new(device.clone());
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &header).unwrap();
        writer
            .write_event(&TraceEvent::new(0, 1000, EV_SYN, 0, 0))
            .unwrap();
        writer.finish().unwrap();
    }
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    ReplayDriver::replay(std::io::Cursor::new(buffer), &mut decoder).unwrap();
    assert_eq!(decoder.into_sink().frames().len(), 1);
}

#[test]
fn replay_rejects_unreasonably_large_slot_count_header() {
    // A replay-controlled header must not be able to request an effectively
    // unbounded allocation: slot counts above the documented maximum are
    // rejected with a structured InvalidDevice error before any decoder state
    // is built (M3 review R6).
    let mut device = type_b_descriptor();
    device.slot_count = Some(1_000_000_000);
    let header = TraceHeader::new(device);
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &header).unwrap();
        writer.finish().unwrap();
    }
    let mut decoder = TypeBDecoder::new(RecordingFrameSink::new());
    let err = ReplayDriver::replay(std::io::Cursor::new(buffer), &mut decoder).unwrap_err();
    assert!(matches!(
        err,
        ReplayError::Sink(ReplayDecodeError::Decode(DecodeError::InvalidDevice(_)))
    ));
    assert!(decoder.into_sink().frames().is_empty());
}

/// Mock resync source that always returns a snapshot.
struct OkResync(KernelStateSnapshot);

/// Mock resync source that always fails.
struct FailingResync(&'static str);

#[derive(Debug, Clone, PartialEq)]
struct MockError(String);

impl std::fmt::Display for MockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MockError {}

impl ResyncSource for OkResync {
    fn snapshot(
        &mut self,
    ) -> Result<KernelStateSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.0.clone())
    }
}

impl ResyncSource for FailingResync {
    fn snapshot(
        &mut self,
    ) -> Result<KernelStateSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(MockError(self.0.to_string())))
    }
}

/// Compile-time guard that the parity test's live path uses a descriptor
/// shape the fixtures really declare (a touchpad-core contract check).
#[test]
fn fixture_descriptors_are_type_b_with_slots() {
    for name in FIXTURES {
        let (header, _) = read_fixture(name);
        assert!(header.device.supports_type_b_mt, "{name}");
        assert!(header.device.slot_count.is_some(), "{name}");
        assert_eq!(header.device.axes.len(), 2, "{name}");
    }
}

/// The raw fixture corpus still reads cleanly through the M2 reader and its
/// headers describe the same device shape the Linux layer will produce.
#[test]
fn fixture_corpus_is_reader_clean_with_linux_axis_ids() {
    for name in FIXTURES {
        let (header, events) = read_fixture(name);
        let _ = DeviceDescriptor::new("unused", 0, 0);
        assert!(
            header
                .device
                .axes
                .contains_key(&touchpad_core::AxisId::new(53)),
            "{name}: X axis key"
        );
        assert!(
            header
                .device
                .axes
                .contains_key(&touchpad_core::AxisId::new(54)),
            "{name}: Y axis key"
        );
        assert!(header.device.validate().is_empty(), "{name}");
        assert!(!events.is_empty(), "{name}");
    }
}
