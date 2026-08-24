//! The raw-event recorder that sits **in front of the decoder** (M5).
//!
//! IMPLEMENTATION_BRIEF §8: "raw recorder 位于 decoder 之前，因此即使 decoder
//! 有 bug，trace 仍保留用于复现的原始输入" — the recorder captures exactly
//! what the device produced, before any decoding, so a decoder bug can never
//! lose the raw input needed to reproduce it.
//!
//! [`RawEventRecorder`] is the runtime's recorder contract: every kernel
//! event decoded from a read batch is handed to the recorder **before** it
//! reaches [`crate::decode::TypeBDecoder::feed`] ([`crate::runtime`] calls
//! [`RawEventRecorder::record`] for each event first, then feeds the
//! decoder). A failure of the recorder is fatal for the session (the runtime
//! fails open), but events already recorded stay recorded.
//!
//! [`TraceRecorder`] is the versioned-JSON-Lines implementation: it converts
//! each [`KernelEvent`] into a [`touchpad_trace::TraceEvent`] (via
//! [`KernelEvent::to_trace_event`]) and appends it to a
//! [`touchpad_trace::TraceWriter`], which writes the mandatory header first
//! and validates every line. Recorder flush/finish delegate to the writer's
//! flush/finish semantics (see [`touchpad_trace::TraceWriter`]): a failed
//! event-line write poisons the writer, and a poisoned recorder rejects every
//! further operation.
//!
//! This module is `unsafe`-free.
#![forbid(unsafe_code)]

use std::io::{BufWriter, Write};
use std::path::Path;

use touchpad_trace::{TraceError, TraceHeader, TraceWriter};

use crate::event::{KernelEvent, TimevalError};

/// Failure of a raw-event recorder.
#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    /// The recorder output could not be created (e.g. the output path is
    /// unwritable).
    #[error("could not create recorder output: {0}")]
    Io(#[from] std::io::Error),
    /// The underlying trace writer failed (I/O error, poisoned stream, ...).
    #[error("trace recorder failed: {0}")]
    Trace(#[from] TraceError),
    /// A kernel event's timeval cannot be represented in the trace schema
    /// (negative or out-of-range `(sec, usec)`); the recorder never writes an
    /// unrepresentable timestamp.
    #[error("recorder cannot represent the event's timeval: {0}")]
    Timeval(#[from] TimevalError),
}

/// Contract for a raw-event recorder attached to the input runtime.
///
/// The runtime calls [`RawEventRecorder::record`] for every kernel event
/// decoded from a read batch, **before** the event is fed to the decoder, so
/// the trace is the ground truth of what the device delivered even when the
/// decoder fails afterwards.
///
/// The ordered finalization (M5 re-review R3) calls
/// [`RawEventRecorder::finish`] — the complete fallible finalization, which
/// flushes — **before** the grab release and fd close, then destroys the
/// recorder so its best-effort `Drop` flush (the last chance to push
/// buffered bytes when `finish` failed) also precedes the device release.
/// [`RawEventRecorder::flush`] remains available as an explicit durability
/// checkpoint (e.g. the record command proves the output is writable by
/// flushing the header).
pub trait RawEventRecorder {
    /// Records one raw kernel event (before the decoder sees it).
    fn record(&mut self, event: &KernelEvent) -> Result<(), RecorderError>;

    /// Pushes buffered data to the underlying sink. Idempotent; used as an
    /// explicit durability checkpoint (e.g. proving the header reached the
    /// file during preparation).
    fn flush(&mut self) -> Result<(), RecorderError>;

    /// Marks the recorder cleanly finished (no further events may be
    /// recorded). For the trace writer this is `TraceWriter::finish`. Called
    /// by the runtime's ordered finalization **before** the device release.
    fn finish(&mut self) -> Result<(), RecorderError>;

    /// The number of raw events recorded so far (for status reporting).
    fn events_recorded(&self) -> u64;
}

/// A [`RawEventRecorder`] that appends raw kernel events to a versioned
/// JSON Lines trace ([`touchpad_trace::TraceWriter`]).
///
/// [`TraceRecorder::create`] opens the output file and writes the mandatory
/// header line **into a `BufWriter`** — the header is buffered, not
/// guaranteed on disk, until an explicit [`RawEventRecorder::flush`] (or
/// `finish`, or the writer's best-effort `Drop` flush) succeeds. Callers
/// that need to *prove* the output is writable before doing anything else
/// (the record command, M5 review R2) must call [`RawEventRecorder::flush`]
/// after `create` and treat a flush failure as an unwritable output.
/// [`TraceRecorder::over`] wraps an existing writer (tests). Recording is
/// streaming: one line per event, constant memory.
pub struct TraceRecorder {
    writer: TraceWriter<Box<dyn Write>>,
    /// Raw events recorded so far.
    events: u64,
}

impl TraceRecorder {
    /// Opens `path` for recording and writes the trace header as the first
    /// line into the buffered writer. Fails without producing a recorder
    /// when the file cannot be created or the header is rejected.
    ///
    /// **Buffering contract (honest):** the header is written into a
    /// `BufWriter`, so a successful `create` does *not* by itself prove the
    /// header reached the file. Call [`RawEventRecorder::flush`] and treat
    /// its success as the proof that the output is writable.
    pub fn create(path: &Path, header: &TraceHeader) -> Result<Self, RecorderError> {
        let file = std::fs::File::create(path)?;
        Self::over(Box::new(BufWriter::new(file)), header)
    }

    /// Wraps an existing writer with a recorder (used by tests; the writer's
    /// header was already written by [`TraceWriter::new`]).
    pub fn over(writer: Box<dyn Write>, header: &TraceHeader) -> Result<Self, RecorderError> {
        Ok(Self {
            writer: TraceWriter::new(writer, header)?,
            events: 0,
        })
    }
}

impl RawEventRecorder for TraceRecorder {
    fn record(&mut self, event: &KernelEvent) -> Result<(), RecorderError> {
        let trace_event = event.to_trace_event()?;
        self.writer.write_event(&trace_event)?;
        self.events += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RecorderError> {
        self.writer.flush()?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RecorderError> {
        self.writer.finish()?;
        Ok(())
    }

    fn events_recorded(&self) -> u64 {
        self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use touchpad_core::DeviceDescriptor;
    use touchpad_trace::{TraceReader, TraceTime};

    use crate::event::KernelEvent;

    /// A unique temp file path per test (parallel-safe enough for tests:
    /// process id + test-name hash + a counter).
    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "touchpadctl-recorder-{}-{}-{}.jsonl",
            std::process::id(),
            unique,
            tag
        ))
    }

    fn header() -> TraceHeader {
        TraceHeader::new(DeviceDescriptor::new("recorder test", 0, 0))
    }

    fn ev(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> KernelEvent {
        KernelEvent {
            sec,
            usec,
            event_type,
            code,
            value,
        }
    }

    #[test]
    fn records_events_before_they_could_reach_the_decoder() {
        let path = temp_path("records");
        let mut recorder = TraceRecorder::create(&path, &header()).unwrap();
        recorder.record(&ev(1, 1000, 3, 53, 100)).unwrap();
        recorder.record(&ev(1, 2000, 0, 0, 0)).unwrap();
        recorder.flush().unwrap();

        // The trace is readable and carries exactly the recorded events, in
        // order, with the raw (sec, usec) pairs preserved.
        let mut reader = TraceReader::new(std::fs::File::open(&path).unwrap());
        let header = reader.read_header().unwrap();
        assert_eq!(header.device.name, "recorder test");
        let events: Vec<_> = reader.events().map(Result::unwrap).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].time(), TraceTime { sec: 1, usec: 1000 });
        assert_eq!(events[1].time(), TraceTime { sec: 1, usec: 2000 });
        assert_eq!(recorder.events_recorded(), 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn finish_marks_a_clean_recording() {
        let path = temp_path("finish");
        let mut recorder = TraceRecorder::create(&path, &header()).unwrap();
        recorder.record(&ev(0, 0, 3, 53, 1)).unwrap();
        recorder.finish().unwrap();
        // finish is the normal end; a second finish is rejected.
        assert!(recorder.finish().is_err());
        // The file is a clean header+event trace.
        let mut reader = TraceReader::new(std::fs::File::open(&path).unwrap());
        reader.read_header().unwrap();
        assert_eq!(reader.events().count(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_unrepresentable_timevals_without_writing() {
        let path = temp_path("bad-timeval");
        let mut recorder = TraceRecorder::create(&path, &header()).unwrap();
        let err = recorder.record(&ev(-1, 0, 3, 53, 0)).unwrap_err();
        assert!(matches!(err, RecorderError::Timeval(_)), "{err:?}");
        // Nothing was recorded and the recorder stays usable.
        assert_eq!(recorder.events_recorded(), 0);
        recorder.record(&ev(0, 0, 3, 53, 1)).unwrap();
        recorder.finish().unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_failure_is_actionable() {
        let err = match TraceRecorder::create(
            Path::new("/definitely/not/a/real/directory/xyz/trace.jsonl"),
            &header(),
        ) {
            Err(err) => err,
            Ok(_) => panic!("expected create to fail"),
        };
        assert!(matches!(err, RecorderError::Io(_)), "{err:?}");
    }
}
