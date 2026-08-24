//! Platform-neutral offline replay boundary.
//!
//! This is the contract that lets a raw trace drive the *same* input
//! processing path as a live device (design.md §15, IMPLEMENTATION_BRIEF
//! §3.3/§8): [`ReplaySink`] is implemented by the raw-event consumer, and
//! the M3 Type-B decoder will implement it with the exact state machine it
//! uses for live input — replay never builds a second model of the device.
//!
//! **M2 deliberately ships no decoder and produces no `ContactFrame`
//! output.** The driver only forwards raw events in trace order; tests
//! observe them with a recording sink ([`RecordingSink`]) that records raw
//! events verbatim. Decoding raw events into normalized frames is M3's
//! scope.

use std::error::Error as StdError;
use std::fmt;
use std::io::Read;

use crate::error::TraceError;
use crate::event::TraceEvent;
use crate::header::TraceHeader;
use crate::reader::TraceReader;
use crate::time::TraceTime;

/// Failure of a [`ReplaySink`] method.
///
/// Consumers map their own failure modes onto this type: an event the sink
/// cannot accept ([`SinkError::Rejected`]) or a fatal condition that must
/// stop the replay ([`SinkError::Fatal`]). `Rejected` and `Fatal` carry the
/// same fail-open meaning as [`touchpad_core::OutputError`]'s rejected/fatal
/// distinction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The sink rejected a specific input (line number is 1-based in the
    /// trace).
    #[error("replay sink rejected input on line {line_number}: {message}")]
    Rejected {
        /// 1-based trace line number of the offending event.
        line_number: u64,
        /// Why the input was rejected.
        message: String,
    },
    /// The sink failed fatally; the replay must stop.
    #[error("replay sink failed fatally: {0}")]
    Fatal(String),
}

/// Platform-neutral boundary for consuming a replayed raw event stream.
///
/// The M3 decoder implements this trait with the same state machine it uses
/// for live input. The driver calls, in order:
///
/// 1. [`ReplaySink::on_header`] once with the trace header (the device
///    descriptor, schema version, and clock of the trace);
/// 2. [`ReplaySink::on_event`] once per raw event, in trace order;
/// 3. [`ReplaySink::finish`] once, only after the whole trace was read
///    cleanly (a corrupt trace never triggers `finish`).
///
/// Wall-clock time never participates: the driver forwards events as fast as
/// it can read them and makes no pacing decisions.
pub trait ReplaySink {
    /// The sink's failure type.
    type Error: StdError + Send + Sync + 'static;

    /// Applies the trace header (schema, clock, device descriptor). Called
    /// exactly once, before the first event.
    fn on_header(&mut self, header: &TraceHeader) -> Result<(), Self::Error>;

    /// Consumes one raw event, in trace order.
    fn on_event(&mut self, event: &TraceEvent) -> Result<(), Self::Error>;

    /// Signals the clean end of the trace. Called only after every event was
    /// forwarded successfully and end of file was reached.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

/// Errors produced by [`ReplayDriver::replay`].
#[derive(Debug)]
pub enum ReplayError<E> {
    /// The trace itself is invalid (read error, schema mismatch, corrupted
    /// line, time regression, ...).
    Trace(TraceError),
    /// The sink rejected or failed on an input.
    Sink(E),
}

impl<E> fmt::Display for ReplayError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::Trace(err) => write!(f, "trace error: {err}"),
            ReplayError::Sink(err) => write!(f, "replay sink error: {err}"),
        }
    }
}

impl<E> StdError for ReplayError<E>
where
    E: StdError + 'static,
{
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            ReplayError::Trace(err) => Some(err),
            ReplayError::Sink(err) => Some(err),
        }
    }
}

impl<E> From<TraceError> for ReplayError<E> {
    fn from(err: TraceError) -> Self {
        ReplayError::Trace(err)
    }
}

/// Summary of a finished replay.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayStats {
    /// The trace header that was applied.
    pub header: TraceHeader,
    /// Number of raw events forwarded to the sink.
    pub events_forwarded: u64,
    /// Timestamp of the first forwarded event.
    pub first_time: Option<TraceTime>,
    /// Timestamp of the last forwarded event.
    pub last_time: Option<TraceTime>,
}

/// Drives a trace through a [`ReplaySink`].
///
/// The driver owns the streaming read: it constructs a [`TraceReader`] over
/// the input, enforces the header-first/once contract, and forwards every
/// raw event to the sink in order. It never decodes events and never reads a
/// wall clock.
pub struct ReplayDriver;

impl ReplayDriver {
    /// Replays `input` through `sink`.
    ///
    /// On success returns the [`ReplayStats`]. On failure returns
    /// [`ReplayError::Trace`] when the trace is invalid (the sink's
    /// `finish` is *not* called in that case) or [`ReplayError::Sink`] when
    /// the sink rejected/failed on an input.
    pub fn replay<R: Read, S: ReplaySink>(
        input: R,
        sink: &mut S,
    ) -> Result<ReplayStats, ReplayError<S::Error>> {
        let mut reader = TraceReader::new(input);
        let header = reader.read_header()?;
        sink.on_header(&header).map_err(ReplayError::Sink)?;

        let mut stats = ReplayStats {
            header,
            events_forwarded: 0,
            first_time: None,
            last_time: None,
        };
        while let Some(event) = reader.read_event()? {
            sink.on_event(&event).map_err(ReplayError::Sink)?;
            stats.events_forwarded += 1;
            if stats.first_time.is_none() {
                stats.first_time = Some(event.time());
            }
            stats.last_time = Some(event.time());
        }
        sink.finish().map_err(ReplayError::Sink)?;
        Ok(stats)
    }
}

/// Misuse of a [`RecordingSink`] (e.g. receiving a second header).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("recording sink misuse: {0}")]
pub struct RecordingSinkError(String);

/// Observation-only sink that records the raw events it receives.
///
/// This is **not a decoder**: it never produces [`touchpad_core::ContactFrame`]
/// output. It exists to demonstrate and test the replay boundary (and to
/// give M3's decoder integration tests an oracle that records raw input
/// verbatim). It records everything and fails only on misuse (a second
/// header).
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    header: Option<TraceHeader>,
    events: Vec<TraceEvent>,
    finished: bool,
}

impl RecordingSink {
    /// Creates an empty recording sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The header received via [`ReplaySink::on_header`], if any.
    #[must_use]
    pub fn header(&self) -> Option<&TraceHeader> {
        self.header.as_ref()
    }

    /// The raw events received so far, in trace order.
    #[must_use]
    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }

    /// Whether [`ReplaySink::finish`] was called.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Number of raw events recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no events were recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl ReplaySink for RecordingSink {
    type Error = RecordingSinkError;

    fn on_header(&mut self, header: &TraceHeader) -> Result<(), Self::Error> {
        if self.header.is_some() {
            return Err(RecordingSinkError(
                "a second header was delivered".to_string(),
            ));
        }
        self.header = Some(header.clone());
        Ok(())
    }

    fn on_event(&mut self, event: &TraceEvent) -> Result<(), Self::Error> {
        self.events.push(event.clone());
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        self.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use touchpad_core::DeviceDescriptor;

    fn header() -> TraceHeader {
        TraceHeader::new(DeviceDescriptor::new("dev", 0, 0))
    }

    fn event(sec: u64, usec: u32, event_type: u16, code: u16, value: i32) -> TraceEvent {
        TraceEvent::new(sec, usec, event_type, code, value)
    }

    fn trace_text() -> String {
        format!(
            "{}\n{}\n{}",
            serde_json::to_string(&crate::TraceLine::Header(header())).unwrap(),
            serde_json::to_string(&crate::TraceLine::Event(event(0, 1000, 3, 47, 0))).unwrap(),
            serde_json::to_string(&crate::TraceLine::Event(event(0, 2000, 0, 0, 0))).unwrap(),
        )
    }

    #[test]
    fn replay_forwards_header_and_events_in_order() {
        let mut sink = RecordingSink::new();
        let stats =
            ReplayDriver::replay(Cursor::new(trace_text().into_bytes()), &mut sink).unwrap();
        assert_eq!(stats.events_forwarded, 2);
        assert_eq!(stats.first_time, Some(TraceTime { sec: 0, usec: 1000 }));
        assert_eq!(stats.last_time, Some(TraceTime { sec: 0, usec: 2000 }));
        assert_eq!(sink.header().unwrap().device.name, "dev");
        assert_eq!(
            sink.events(),
            &[event(0, 1000, 3, 47, 0), event(0, 2000, 0, 0, 0)]
        );
        assert!(sink.is_finished());
    }

    #[test]
    fn replay_reports_trace_errors_without_calling_finish() {
        let text = format!(
            "{}\n{}\n{}",
            serde_json::to_string(&crate::TraceLine::Header(header())).unwrap(),
            serde_json::to_string(&crate::TraceLine::Event(event(0, 2000, 3, 47, 0))).unwrap(),
            serde_json::to_string(&crate::TraceLine::Event(event(0, 1000, 3, 47, 0))).unwrap(),
        );
        let mut sink = RecordingSink::new();
        let err = ReplayDriver::replay(Cursor::new(text.into_bytes()), &mut sink).unwrap_err();
        assert!(matches!(
            err,
            ReplayError::Trace(TraceError::TimeRegression { .. })
        ));
        // The trace was invalid: `finish` must not have been called.
        assert!(!sink.is_finished());
        assert_eq!(sink.events().len(), 1);
    }

    #[test]
    fn replay_reports_sink_errors() {
        struct RejectingSink;

        impl ReplaySink for RejectingSink {
            type Error = SinkError;

            fn on_header(&mut self, _header: &TraceHeader) -> Result<(), Self::Error> {
                Ok(())
            }

            fn on_event(&mut self, event: &TraceEvent) -> Result<(), Self::Error> {
                Err(SinkError::Rejected {
                    line_number: 2,
                    message: format!("cannot accept event {event:?}"),
                })
            }

            fn finish(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }
        }

        let mut sink = RejectingSink;
        let err =
            ReplayDriver::replay(Cursor::new(trace_text().into_bytes()), &mut sink).unwrap_err();
        assert!(matches!(
            err,
            ReplayError::Sink(SinkError::Rejected { line_number: 2, .. })
        ));
    }

    #[test]
    fn recording_sink_rejects_second_header() {
        let mut sink = RecordingSink::new();
        sink.on_header(&header()).unwrap();
        let err = sink.on_header(&header()).unwrap_err();
        assert!(err.to_string().contains("second header"));
    }
}
