//! Streaming JSON Lines trace writer.
//!
//! The writer guarantees the file contract structurally:
//!
//! * the header is written **first and exactly once** — it is written
//!   immediately by [`TraceWriter::new`], and there is no API to write
//!   another header or to write an event before it;
//! * each event line is submitted to the underlying writer as one unit
//!   (`Write::write_all`), so lines are never interleaved by this crate;
//! * `schema_version`/`clock` of the header are validated before writing;
//! * event field ranges are validated on every [`TraceWriter::write_event`].
//!
//! ## Failure semantics: validation vs. I/O
//!
//! Failures are split into two classes with deliberately different
//! consequences:
//!
//! * **Pre-write failures leave the writer usable.** Field validation
//!   ([`TraceEvent::validate_fields`]) and line serialization happen before
//!   any byte reaches the underlying writer. A rejection there means nothing
//!   was written, so the caller may retry with a corrected event, flush, or
//!   finish.
//! * **Event-line I/O failures poison the writer.** A generic
//!   [`Write::write_all`] may return an error after writing a *prefix* of a
//!   line. From that point on the stream may contain a partial line, so the
//!   writer enters a terminal **poisoned** state ([`TraceError::Poisoned`]):
//!   [`TraceWriter::write_event`], [`TraceWriter::flush`], and
//!   [`TraceWriter::finish`] all fail deterministically. A retry would append
//!   another JSON object to the partial prefix and silently corrupt the
//!   trace, so retry is never implied to be safe. The one I/O error is
//!   reported as [`TraceError::Io`]; subsequent calls report
//!   [`TraceError::Poisoned`].
//!
//! Note that this crate cannot make `Write` implementations atomic: it
//! *always* submits a whole line via `write_all`, but an underlying writer
//! that fails mid-line can still leave a partial line behind. Hence a trace
//! written by this crate is a valid JSON Lines stream only up to the last
//! line whose write completed.
//!
//! ## Recording fidelity
//!
//! The writer records raw input **faithfully** (IMPLEMENTATION_BRIEF §8: the
//! recorder sits in front of the decoder so a trace always preserves the
//! original input). It validates field ranges (an invalid `usec` or an
//! unconvertible timestamp is a bug in the caller and is rejected), but it
//! deliberately does **not** reject or rewrite non-monotonic timestamps: a
//! real-but-odd kernel timestamp must be preserved for regression
//! reproduction. The reader enforces the non-decreasing policy at replay
//! time ([`crate::TraceError::TimeRegression`]). Consequently, **not every
//! trace this writer produces is accepted by its own reader**: a capture
//! containing a regressed timestamp is a faithfully recorded but
//! replay-invalid diagnostic artifact (see `tests/roundtrip.rs`). The header
//! itself is always reader-acceptable because it is validated up front.
//!
//! ## Flush / finish semantics
//!
//! * [`TraceWriter::flush`] pushes buffered bytes to the underlying writer
//!   and surfaces I/O errors. A failed flush does not poison the writer (no
//!   partial *line* can be produced by a flush; the underlying sink may have
//!   its own durability semantics), but `flush` on a poisoned writer is
//!   rejected.
//! * [`TraceWriter::finish`] is the **normal end**: it flushes and marks the
//!   writer finished. Calling `finish` twice, or writing after `finish`,
//!   is a programming error and is reported as
//!   [`TraceError::InvalidState`]. `finish` on a poisoned writer is rejected
//!   with [`TraceError::Poisoned`].
//! * Dropping a writer without calling `finish` is an **abnormal end** (e.g.
//!   an early error return). `Drop` still best-effort flushes whatever was
//!   written, so a partially recorded trace remains readable up to the last
//!   flushed line — but callers must treat only a `finish`ed trace as a
//!   clean recording.

use std::io::Write;

use crate::error::TraceError;
use crate::event::TraceEvent;
use crate::header::TraceClock;
use crate::header::TraceHeader;
use crate::TraceLine;
use crate::SUPPORTED_SCHEMA_VERSION;

/// Streaming JSON Lines trace writer.
///
/// Usage:
///
/// ```no_run
/// # use std::fs::File;
/// # use touchpad_core::DeviceDescriptor;
/// # use touchpad_trace::{TraceHeader, TraceWriter, TraceEvent};
/// let header = TraceHeader::new(DeviceDescriptor::new("dev", 0, 0));
/// let file = File::create("trace.jsonl").unwrap();
/// let mut writer = TraceWriter::new(file, &header).unwrap();
/// writer.write_event(&TraceEvent::new(0, 1000, 3, 47, 0)).unwrap();
/// writer.finish().unwrap();
/// ```
#[derive(Debug)]
pub struct TraceWriter<W: Write> {
    inner: W,
    /// Set by [`TraceWriter::finish`]; writing after finish is an error.
    finished: bool,
    /// Set after an event-line I/O failure that may have written a partial
    /// line. A poisoned writer rejects every further operation.
    poisoned: bool,
    /// 1-based line number of the next line to be written (header is 1).
    next_line: u64,
}

impl<W: Write> TraceWriter<W> {
    /// Creates a writer and immediately writes the header as the first line.
    ///
    /// The header is validated (supported `schema_version`, monotonic
    /// clock) before anything is written. The header is written eagerly, not
    /// buffered: a trace file never exists without its header line. A header
    /// this method accepts is always acceptable to the reader (schema and
    /// clock are validated); the *event* stream is a different matter — the
    /// writer records regressed timestamps faithfully, so a trace it
    /// produces may still be rejected by the reader with
    /// [`TraceError::TimeRegression`] (see the module docs).
    pub fn new(mut inner: W, header: &TraceHeader) -> Result<Self, TraceError> {
        validate_header(header)?;
        let line = serialize_line(&TraceLine::Header(header.clone()))?;
        inner.write_all(line.as_bytes()).map_err(TraceError::Io)?;
        inner.write_all(b"\n").map_err(TraceError::Io)?;
        Ok(Self {
            inner,
            finished: false,
            poisoned: false,
            next_line: 2,
        })
    }

    /// Appends one raw event line.
    ///
    /// Validates the event's field ranges (see [`TraceEvent::validate_fields`])
    /// but not timestamp ordering — recording is faithful.
    ///
    /// Failure semantics are split precisely (see the module docs):
    ///
    /// * a **field-validation or serialization failure** happens before any
    ///   byte is written; the writer remains usable and the caller may retry
    ///   with a corrected event;
    /// * an **I/O failure while writing the line** may have written a partial
    ///   line; the writer is poisoned and every subsequent `write_event`,
    ///   `flush`, and `finish` fails with [`TraceError::Poisoned`]. Retrying
    ///   is never safe after that point.
    pub fn write_event(&mut self, event: &TraceEvent) -> Result<(), TraceError> {
        if self.poisoned {
            return Err(TraceError::Poisoned(
                "write_event on a poisoned writer: a partial line may have been written",
            ));
        }
        if self.finished {
            return Err(TraceError::InvalidState("write_event called after finish"));
        }
        event
            .validate_fields()
            .map_err(|err| TraceError::InvalidField {
                line_number: self.next_line,
                message: err.to_string(),
            })?;
        let line = serialize_line(&TraceLine::Event(event.clone()))?;
        if let Err(err) = self.inner.write_all(line.as_bytes()) {
            self.poisoned = true;
            return Err(TraceError::Io(err));
        }
        if let Err(err) = self.inner.write_all(b"\n") {
            self.poisoned = true;
            return Err(TraceError::Io(err));
        }
        self.next_line += 1;
        Ok(())
    }

    /// Flushes buffered bytes to the underlying writer.
    ///
    /// Useful for recorders that want the trace durable even before the
    /// recording session ends. A failed flush surfaces the I/O error without
    /// poisoning the writer (a flush cannot create a partial *line*), but
    /// flushing a poisoned writer is rejected.
    pub fn flush(&mut self) -> Result<(), TraceError> {
        if self.poisoned {
            return Err(TraceError::Poisoned(
                "flush on a poisoned writer: the stream may contain a partial line",
            ));
        }
        self.inner.flush().map_err(TraceError::Io)
    }

    /// Normal end of the trace: flushes and marks the writer finished.
    ///
    /// Calling `finish` twice, or writing after `finish`, is reported as
    /// [`TraceError::InvalidState`]; `finish` on a poisoned writer is
    /// rejected with [`TraceError::Poisoned`] without attempting another
    /// flush. Dropping a [`TraceWriter`] without calling `finish` is an
    /// abnormal end (see the module docs for the exact flush/finish
    /// semantics).
    pub fn finish(&mut self) -> Result<(), TraceError> {
        if self.poisoned {
            return Err(TraceError::Poisoned(
                "finish on a poisoned writer: the stream may contain a partial line",
            ));
        }
        if self.finished {
            return Err(TraceError::InvalidState("finish called twice"));
        }
        self.flush()?;
        self.finished = true;
        Ok(())
    }
}

impl<W: Write> Drop for TraceWriter<W> {
    fn drop(&mut self) {
        // Best-effort flush on any drop path (including abnormal ends), so a
        // partially recorded trace is preserved up to the last written line.
        // Errors cannot be reported from Drop; callers that need error
        // reporting must call `flush`/`finish` explicitly.
        let _ = self.inner.flush();
    }
}

/// Validates a header before writing: supported schema version and monotonic
/// clock.
fn validate_header(header: &TraceHeader) -> Result<(), TraceError> {
    let found = u64::from(header.schema_version);
    if found > u64::from(SUPPORTED_SCHEMA_VERSION) {
        return Err(TraceError::SchemaTooNew {
            found,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }
    if found < u64::from(SUPPORTED_SCHEMA_VERSION) {
        return Err(TraceError::SchemaTooOld {
            found,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }
    if header.clock != TraceClock::Monotonic {
        return Err(TraceError::InvalidField {
            line_number: 1,
            message: format!(
                "unsupported clock {:?}; schema version 1 defines only \"monotonic\"",
                header.clock
            ),
        });
    }
    Ok(())
}

/// Serializes one trace line to its JSON string. Serialization of the schema
/// types cannot fail in practice; a failure is surfaced as a structured
/// error rather than a panic.
fn serialize_line<T: serde::Serialize>(value: &T) -> Result<String, TraceError> {
    serde_json::to_string(value).map_err(|err| TraceError::Serialize(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchpad_core::DeviceDescriptor;

    fn header() -> TraceHeader {
        TraceHeader::new(DeviceDescriptor::new("dev", 0x1234, 0x5678))
    }

    fn event(sec: u64, usec: u32, event_type: u16, code: u16, value: i32) -> TraceEvent {
        TraceEvent::new(sec, usec, event_type, code, value)
    }

    fn write_all<W: Write>(writer: &mut TraceWriter<W>, events: &[TraceEvent]) {
        for e in events {
            writer.write_event(e).unwrap();
        }
    }

    #[test]
    fn writes_header_first_then_events() {
        let mut buffer = Vec::new();
        {
            let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
            write_all(
                &mut writer,
                &[event(0, 1000, 3, 47, 0), event(0, 2000, 0, 0, 0)],
            );
            writer.finish().unwrap();
        }
        let text = String::from_utf8(buffer).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"kind\":\"header\""));
        assert!(lines[0].contains("\"schema_version\":1"));
        assert!(lines[1].contains("\"kind\":\"event\""));
        assert!(lines[2].contains("\"kind\":\"event\""));
    }

    #[test]
    fn rejects_header_with_newer_schema() {
        let mut header = header();
        header.schema_version = 2;
        let mut buffer = Vec::new();
        let err = TraceWriter::new(&mut buffer, &header).unwrap_err();
        assert!(matches!(
            err,
            TraceError::SchemaTooNew {
                found: 2,
                supported: 1
            }
        ));
        // Nothing was written for a rejected header.
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_invalid_event_fields() {
        let mut buffer = Vec::new();
        let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
        let err = writer
            .write_event(&event(0, 1_000_000, 3, 47, 0))
            .unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, .. }
        ));
        // The writer stays usable after a rejected event.
        writer.write_event(&event(0, 1000, 3, 47, 0)).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn finish_then_write_is_invalid_state() {
        let mut buffer = Vec::new();
        let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
        writer.finish().unwrap();
        let err = writer.write_event(&event(0, 1000, 3, 47, 0)).unwrap_err();
        assert!(matches!(err, TraceError::InvalidState(_)));
    }

    #[test]
    fn finish_twice_is_invalid_state() {
        let mut buffer = Vec::new();
        let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
        writer.finish().unwrap();
        let err = writer.finish().unwrap_err();
        assert!(matches!(err, TraceError::InvalidState(_)));
    }

    #[test]
    fn flush_emits_buffered_bytes() {
        let mut buffer = Vec::new();
        {
            let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
            writer.write_event(&event(0, 1000, 3, 47, 0)).unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(std::str::from_utf8(&buffer).unwrap().lines().count(), 2);
    }

    #[test]
    fn drop_without_finish_preserves_written_lines() {
        let mut buffer = Vec::new();
        {
            let mut writer = TraceWriter::new(&mut buffer, &header()).unwrap();
            writer.write_event(&event(0, 1000, 3, 47, 0)).unwrap();
            // No finish(): abnormal end, but Drop flushes best-effort.
        }
        assert_eq!(std::str::from_utf8(&buffer).unwrap().lines().count(), 2);
    }
}

#[cfg(test)]
mod poisoning_tests {
    use super::*;
    use touchpad_core::DeviceDescriptor;

    /// A `Write` that accepts exactly `limit` bytes and then fails every
    /// further write — the fault injector that proves a partial event line
    /// can reach the sink and that the writer poisons itself afterwards.
    #[derive(Debug)]
    struct FailAfter {
        written: Vec<u8>,
        limit: usize,
    }

    impl Write for FailAfter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.written.len() >= self.limit {
                return Err(std::io::Error::other("injected write failure"));
            }
            let remaining = self.limit - self.written.len();
            let take = buf.len().min(remaining);
            self.written.extend_from_slice(&buf[..take]);
            Ok(take)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Serializes a header/event line the same way the writer does, for byte
    /// accounting in the fault-injection tests.
    fn line_json(line: &crate::TraceLine) -> String {
        serde_json::to_string(line).unwrap()
    }

    fn header() -> TraceHeader {
        TraceHeader::new(DeviceDescriptor::new("dev", 0x1234, 0x5678))
    }

    fn event() -> TraceEvent {
        TraceEvent::new(0, 1000, 3, 47, 0)
    }

    #[test]
    fn header_write_failure_in_new_returns_error_without_writer() {
        // A header write failure happens inside `new`, so no writer object is
        // ever handed out: there is no state to poison.
        let mut sink = FailAfter {
            written: Vec::new(),
            limit: 0,
        };
        let err = TraceWriter::new(&mut sink, &header()).unwrap_err();
        assert!(matches!(err, TraceError::Io(_)));
        assert!(sink.written.is_empty());
    }

    #[test]
    fn io_failure_mid_event_line_poisons_writer() {
        let header = header();
        let event = event();
        let header_json = line_json(&crate::TraceLine::Header(header.clone()));
        let event_line = line_json(&crate::TraceLine::Event(event.clone()));
        // Let the header through, then fail halfway through the first event
        // line. Event lines are pure ASCII, so a byte midpoint is a clean
        // split.
        let fail_after = header_json.len() + 1 + event_line.len() / 2;
        let mut sink = FailAfter {
            written: Vec::new(),
            limit: fail_after,
        };
        {
            let mut writer = TraceWriter::new(&mut sink, &header).unwrap();

            let err = writer.write_event(&event).unwrap_err();
            assert!(matches!(err, TraceError::Io(_)));

            // The writer is poisoned: no retry, no flush, no finish may
            // proceed as though the trace were clean.
            assert!(matches!(
                writer.write_event(&event),
                Err(TraceError::Poisoned(_))
            ));
            assert!(matches!(writer.flush(), Err(TraceError::Poisoned(_))));
            assert!(matches!(writer.finish(), Err(TraceError::Poisoned(_))));
            // `writer` drops here; its borrow of `sink` ends.
        }

        // Proof that a partial line was written: the sink holds the header,
        // its newline, and a strict prefix of the event line. A generic
        // `Write::write_all` can indeed fail after a prefix.
        assert_eq!(
            &sink.written[header_json.len() + 1..],
            &event_line.as_bytes()[..event_line.len() / 2],
            "the sink must contain a partial event line"
        );

        // The resulting stream is not a clean JSON Lines stream: the final
        // "line" is a truncated JSON object.
        let text = String::from_utf8(sink.written).unwrap();
        let last_line = text.lines().last().expect("a partial line exists");
        assert!(
            serde_json::from_str::<serde_json::Value>(last_line).is_err(),
            "last line must be a partial (invalid) JSON object: {last_line:?}"
        );
    }

    #[test]
    fn io_failure_on_line_terminator_poisons_writer() {
        let header = header();
        let event = event();
        let header_json = line_json(&crate::TraceLine::Header(header.clone()));
        let event_line = line_json(&crate::TraceLine::Event(event.clone()));
        // Fail after the complete event JSON but before its newline: the line
        // still lacks its terminator, so the writer must poison itself.
        let fail_after = header_json.len() + 1 + event_line.len();
        let mut sink = FailAfter {
            written: Vec::new(),
            limit: fail_after,
        };
        {
            let mut writer = TraceWriter::new(&mut sink, &header).unwrap();

            let err = writer.write_event(&event).unwrap_err();
            assert!(matches!(err, TraceError::Io(_)));
            assert!(matches!(
                writer.write_event(&event),
                Err(TraceError::Poisoned(_))
            ));
            assert!(matches!(writer.finish(), Err(TraceError::Poisoned(_))));
        }
        // The full event JSON was written but its newline was not: the last
        // line is a valid JSON object that is missing its terminator.
        assert_eq!(
            &sink.written[header_json.len() + 1..],
            event_line.as_bytes(),
            "the full event JSON must have been written without its newline"
        );
    }
}
