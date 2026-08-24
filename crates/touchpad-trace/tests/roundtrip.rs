//! Streaming round-trip tests.
//!
//! For **replay-accepted** (non-regressed) event streams, whatever the writer
//! emits the reader must read back exactly, and large traces must flow
//! through the line-by-line path without being loaded wholesale into memory.
//!
//! The one deliberate exception is timestamp regression, per the recording
//! fidelity policy (IMPLEMENTATION_BRIEF §8, DESIGN_V2 §5): the writer
//! preserves a real-but-regressed kernel timestamp, while the reader
//! diagnoses `TimeRegression`. So a trace the writer produces successfully is
//! not *always* accepted by its own reader — the end-to-end case is tested in
//! [`regressed_capture_is_preserved_by_writer_but_rejected_by_reader`].

use std::io::Cursor;

use touchpad_core::DeviceDescriptor;
use touchpad_trace::{
    RecordingSink, ReplayDriver, ReplayError, TraceError, TraceEvent, TraceHeader, TraceReader,
    TraceWriter,
};

fn sample_header() -> TraceHeader {
    let mut descriptor = DeviceDescriptor::new("round-trip dev", 0x1234, 0x5678);
    descriptor.slot_count = Some(10);
    descriptor.supports_type_b_mt = true;
    descriptor.has_physical_buttons = true;
    TraceHeader::new(descriptor)
}

fn sample_event(seq: u64) -> TraceEvent {
    // A small synthetic kernel-style event stream: slot select, tracking id,
    // position, SYN_REPORT.
    let usec = (seq * 1000) as u32;
    let code = match seq % 4 {
        0 => 47, // ABS_MT_SLOT
        1 => 57, // ABS_MT_TRACKING_ID
        2 => 53, // ABS_MT_POSITION_X
        _ => 0,  // SYN_REPORT
    };
    TraceEvent::new(0, usec, 3, code, (seq % 500) as i32)
}

fn round_trip(events: &[TraceEvent]) -> (TraceHeader, Vec<TraceEvent>) {
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &sample_header()).unwrap();
        for event in events {
            writer.write_event(event).unwrap();
        }
        writer.finish().unwrap();
    }
    let mut reader = TraceReader::new(Cursor::new(buffer));
    let header = reader.read_header().unwrap();
    let read_back: Vec<TraceEvent> = reader.events().map(Result::unwrap).collect();
    (header, read_back)
}

#[test]
fn small_trace_round_trips_exactly() {
    let events: Vec<TraceEvent> = (0..10).map(sample_event).collect();
    let (header, read_back) = round_trip(&events);
    assert_eq!(header, sample_header());
    assert_eq!(read_back, events);
}

#[test]
fn empty_event_list_round_trips_as_header_only_trace() {
    let (header, read_back) = round_trip(&[]);
    assert_eq!(header, sample_header());
    assert!(read_back.is_empty());
}

#[test]
fn large_trace_is_streamed_line_by_line() {
    // 200k events: the writer emits one line at a time and the reader
    // consumes one line at a time; neither ever holds the whole trace (the
    // test only keeps a counter, not the events). Times roll over into the
    // next second every 1000 events and must stay non-decreasing.
    const COUNT: u64 = 200_000;
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &sample_header()).unwrap();
        for seq in 0..COUNT {
            let sec = seq / 1000;
            let usec = ((seq % 1000) * 1000) as u32;
            writer
                .write_event(&TraceEvent::new(sec, usec, 3, 47, (seq % 500) as i32))
                .unwrap();
        }
        writer.finish().unwrap();
    }

    let mut reader = TraceReader::new(Cursor::new(buffer));
    let header = reader.read_header().unwrap();
    assert_eq!(header, sample_header());

    let mut seen: u64 = 0;
    let mut last_time = None;
    while let Some(event) = reader.read_event().unwrap() {
        // Times must stay non-decreasing across the whole stream (the
        // reader enforces it; assert we actually walked every line).
        if let Some(prev) = last_time {
            assert!(event.time() >= prev);
        }
        last_time = Some(event.time());
        seen += 1;
    }
    assert_eq!(seen, COUNT);
    assert_eq!(
        last_time,
        Some(touchpad_trace::TraceTime {
            sec: (COUNT - 1) / 1000,
            usec: (((COUNT - 1) % 1000) * 1000) as u32,
        })
    );
}

#[test]
fn writer_output_is_valid_json_lines() {
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &sample_header()).unwrap();
        writer.write_event(&sample_event(0)).unwrap();
        writer.flush().unwrap();
    }
    let text = String::from_utf8(buffer).unwrap();
    for line in text.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("line is not valid JSON: {e}: {line:?}"));
    }
}

#[test]
fn every_fixture_round_trips_through_writer_and_reader() {
    // Read a fixture with the reader, write it with the writer, read it
    // back: the header and every event must survive the full cycle. This
    // proves fixtures produced by hand are exactly representable by this
    // crate's writer (same schema).
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for entry in std::fs::read_dir(fixture_dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let mut reader = TraceReader::new(std::fs::File::open(&path).unwrap());
        let header = reader.read_header().unwrap();
        let events: Vec<TraceEvent> = reader.events().map(Result::unwrap).collect();

        let mut buffer = Vec::new();
        {
            let mut writer = TraceWriter::new(&mut buffer, &header).unwrap();
            for event in &events {
                writer.write_event(event).unwrap();
            }
            writer.finish().unwrap();
        }

        let mut reader = TraceReader::new(Cursor::new(buffer));
        let header2 = reader.read_header().unwrap();
        let events2: Vec<TraceEvent> = reader.events().map(Result::unwrap).collect();
        assert_eq!(header2, header, "{name}: header round-trip");
        assert_eq!(events2, events, "{name}: events round-trip");
    }
}

#[test]
fn regressed_capture_is_preserved_by_writer_but_rejected_by_reader() {
    // Recording fidelity (IMPLEMENTATION_BRIEF §8): the writer preserves a
    // real-but-regressed kernel timestamp verbatim; the reader diagnoses it.
    // This is the one documented case where successful writer output is not
    // replay-accepted.
    let mut buffer = Vec::new();
    {
        let mut writer = TraceWriter::new(&mut buffer, &sample_header()).unwrap();
        writer
            .write_event(&TraceEvent::new(0, 2000, 3, 47, 0))
            .unwrap();
        writer
            .write_event(&TraceEvent::new(0, 1000, 3, 47, 0))
            .unwrap();
        writer.finish().unwrap();
    }

    // The written text preserves both raw timestamps, including the
    // regression, exactly as captured (nothing normalized or dropped).
    let text = String::from_utf8(buffer.clone()).unwrap();
    assert!(text.contains("\"usec\":2000"), "first timestamp preserved");
    assert!(
        text.contains("\"usec\":1000"),
        "regressed timestamp preserved"
    );

    // The reader accepts the first event and diagnoses the regression at the
    // exact offending line (3).
    let mut reader = TraceReader::new(Cursor::new(buffer.clone()));
    reader.read_header().unwrap();
    assert!(reader.read_event().unwrap().is_some());
    let err = reader.read_event().unwrap_err();
    assert!(matches!(
        err,
        TraceError::TimeRegression { line_number: 3, .. }
    ));

    // Replay reports the same trace error and never calls `finish`.
    let mut sink = RecordingSink::new();
    let err = ReplayDriver::replay(Cursor::new(buffer), &mut sink).unwrap_err();
    assert!(matches!(
        err,
        ReplayError::Trace(TraceError::TimeRegression { line_number: 3, .. })
    ));
    assert!(!sink.is_finished());
}
