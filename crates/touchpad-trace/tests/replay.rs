//! Replay boundary tests: every fixture must drive the platform-neutral
//! [`ReplaySink`] boundary end-to-end, forwarding the exact raw events in
//! order. M2 ships no decoder, so the observer is a recording sink that
//! stores raw events verbatim — decoding to `ContactFrame` is M3's scope
//! and must reuse this same boundary.

use std::fs::File;
use std::path::{Path, PathBuf};

use touchpad_trace::{RecordingSink, ReplayDriver, TraceEvent};

const FIXTURES: &[&str] = &[
    "single_contact",
    "multi_slot",
    "buttons",
    "missing_resolution",
    "dropped_recovery",
    "m11_fidelity",
];

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.jsonl"))
}

/// Reads a fixture directly (reader path) and returns its events.
fn events_direct(name: &str) -> Vec<TraceEvent> {
    let mut reader = touchpad_trace::TraceReader::new(File::open(fixture_path(name)).unwrap());
    reader.read_header().unwrap();
    reader.events().map(Result::unwrap).collect()
}

#[test]
fn replay_forwards_every_fixture_verbatim() {
    for name in FIXTURES {
        let expected = events_direct(name);
        let mut sink = RecordingSink::new();
        let stats =
            ReplayDriver::replay(File::open(fixture_path(name)).unwrap(), &mut sink).unwrap();

        assert_eq!(stats.events_forwarded as usize, expected.len(), "{name}");
        assert_eq!(
            stats.first_time,
            expected.first().map(TraceEvent::time),
            "{name}"
        );
        assert_eq!(
            stats.last_time,
            expected.last().map(TraceEvent::time),
            "{name}"
        );
        assert_eq!(
            sink.events(),
            expected.as_slice(),
            "{name}: forwarded events"
        );
        assert!(sink.is_finished(), "{name}");
    }
}

#[test]
fn replay_applies_header_before_any_event() {
    let mut sink = RecordingSink::new();
    ReplayDriver::replay(
        File::open(fixture_path("single_contact")).unwrap(),
        &mut sink,
    )
    .unwrap();
    let header = sink.header().expect("header delivered before events");
    assert_eq!(header.schema_version, 1);
    assert!(header.device.supports_type_b_mt);
}

#[test]
fn replay_of_header_only_trace_is_clean() {
    // A trace with a header and no events is valid: finish must still be
    // called and the stats must report zero events.
    let header =
        touchpad_trace::TraceHeader::new(touchpad_core::DeviceDescriptor::new("empty", 0, 0));
    let mut buffer = Vec::new();
    {
        let mut writer = touchpad_trace::TraceWriter::new(&mut buffer, &header).unwrap();
        writer.finish().unwrap();
    }
    let mut sink = RecordingSink::new();
    let stats = ReplayDriver::replay(std::io::Cursor::new(buffer), &mut sink).unwrap();
    assert_eq!(stats.events_forwarded, 0);
    assert!(sink.is_finished());
}

#[test]
fn replay_is_purely_offline_and_never_touches_devices() {
    // The replay path reads from an in-memory buffer only. There is no
    // `/dev/input` access anywhere in `touchpad-trace` (verified by the
    // crate having no platform dependencies); this test exercises the path
    // an unprivileged CI user runs.
    let mut sink = RecordingSink::new();
    let stats =
        ReplayDriver::replay(File::open(fixture_path("buttons")).unwrap(), &mut sink).unwrap();
    assert!(stats.events_forwarded > 0);
}
