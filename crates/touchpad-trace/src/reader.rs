//! Streaming JSON Lines trace reader.
//!
//! The reader processes one line at a time through a [`BufReader`], so a
//! large trace never needs to be loaded into memory: peak memory use is one
//! line. It enforces the file contract:
//!
//! * the first line must be the header ([`TraceError::EmptyTrace`] /
//!   [`TraceError::MissingHeader`]);
//! * the header appears exactly once ([`TraceError::DuplicateHeader`]);
//! * `schema_version` is supported ([`TraceError::SchemaTooNew`] /
//!   [`TraceError::SchemaTooOld`]);
//! * the clock is [`TraceClock::Monotonic`] ([`TraceError::InvalidField`]);
//! * event times are non-decreasing ([`TraceError::TimeRegression`]) and
//!   field ranges hold ([`TraceError::InvalidField`]);
//! * unknown line kinds are rejected ([`TraceError::UnknownLineKind`]) while
//!   unknown optional *fields* are ignored (forward compatibility).
//!
//! ## Terminal failure contract
//!
//! Reading is **fail-stop**: whenever consuming, parsing, or validating a
//! trace line fails — including an underlying I/O failure — the reader enters
//! a terminal **failed** state and every subsequent header/event operation
//! returns [`TraceError::Poisoned`] instead of resuming after the offending
//! line. The original failure is reported exactly once. Consequences:
//!
//! * after a failed `read_header`, a header on a later line is never
//!   accepted (the "first line must be the header" invariant cannot be
//!   bypassed);
//! * after a corrupted line or a time regression, later events are never
//!   consumed.
//!
//! API-misuse errors ([`TraceError::InvalidState`], e.g. `read_event` before
//! `read_header`) do **not** consume a line and therefore do not poison the
//! reader.
//!
//! ## Numeric field classification
//!
//! Integer fields are never deserialized directly into their narrow Rust
//! types (`u64`, `u16`, `i32`, ...): that would let serde reject
//! out-of-range or negative values as generic parse errors before validation.
//! Instead every integer field is read as a raw [`serde_json::Number`] and
//! classified explicitly:
//!
//! * a present **non-number** value (string, bool, `null`, object, array) or
//!   a missing field is `CorruptedLine` (wrong-shaped line);
//! * a present **number that is not an integer** (fractional or exponent
//!   form, e.g. `1.5` or `1e3`) is `InvalidField` (a numeric field with
//!   invalid integrality — never silently truncated);
//! * a present **integer outside the declared range** (negative `sec`,
//!   `usec >= 1_000_000`, `type`/`code` outside `[0, 65535]`, `value`
//!   outside `i32`, ...) is `InvalidField`;
//! * a positive integral `schema_version` representable in `u64` and newer
//!   than supported — including values above `i64::MAX` — is
//!   `SchemaTooNew`, never `CorruptedLine`.

use std::io::{BufRead, BufReader, Read};

use serde::Deserialize;
use serde_json::{Map, Value};

use touchpad_core::DeviceDescriptor;

use crate::error::TraceError;
use crate::event::TraceEvent;
use crate::header::{TraceClock, TraceHeader};
use crate::time::TraceTime;
use crate::SUPPORTED_SCHEMA_VERSION;

/// Reader state: the header must be read first and exactly once, then events
/// until end of file. Any failure while consuming/parsing/validating a line
/// moves the reader to [`ReaderState::Failed`], which is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReaderState {
    /// No line has been consumed; the next line must be the header.
    AwaitingHeader,
    /// The header was consumed; the next lines must be events.
    ReadingEvents,
    /// End of file was reached; further reads return `Ok(None)`.
    Finished,
    /// A line failed to consume/parse/validate (or I/O failed); every
    /// further header/event operation returns [`TraceError::Poisoned`] so a
    /// caller can never resume after the offending line.
    Failed,
}

/// Streaming reader for a JSON Lines raw trace.
///
/// Usage:
///
/// ```no_run
/// # use std::fs::File;
/// # use touchpad_trace::TraceReader;
/// let mut reader = TraceReader::new(File::open("trace.jsonl").unwrap());
/// let header = reader.read_header().unwrap();
/// while let Some(event) = reader.read_event().unwrap() {
///     // forward `event` to the replay sink / decoder.
/// }
/// ```
///
/// [`TraceReader::read_header`] must be called before
/// [`TraceReader::read_event`]; violating the order is reported as
/// [`TraceError::InvalidState`].
pub struct TraceReader<R: Read> {
    inner: BufReader<R>,
    /// 1-based number of the next line to be read.
    line_number: u64,
    state: ReaderState,
    /// Time of the last event read, for the non-decreasing policy.
    last_time: Option<TraceTime>,
}

impl<R: Read> TraceReader<R> {
    /// Wraps a reader. The header is consumed from this same stream by
    /// [`TraceReader::read_header`]; no line is read eagerly.
    #[must_use]
    pub fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
            line_number: 1,
            state: ReaderState::AwaitingHeader,
            last_time: None,
        }
    }

    /// Reads and validates the mandatory first header line.
    ///
    /// Fails with [`TraceError::EmptyTrace`] on an empty file,
    /// [`TraceError::MissingHeader`] when the first line is not a header,
    /// [`TraceError::SchemaTooNew`] / [`TraceError::SchemaTooOld`] on a
    /// mismatched `schema_version`, and [`TraceError::InvalidField`] on an
    /// unsupported clock. May only be called once, before any
    /// [`TraceReader::read_event`].
    ///
    /// Any failure that consumed line 1 is terminal: the reader enters its
    /// failed state and a header on a later line is never accepted (see the
    /// module docs).
    pub fn read_header(&mut self) -> Result<TraceHeader, TraceError> {
        if self.state == ReaderState::Failed {
            return Err(TraceError::Poisoned(
                "read_header on a failed reader: a previous line failed to parse/validate",
            ));
        }
        if self.state != ReaderState::AwaitingHeader {
            return Err(TraceError::InvalidState(
                "read_header called after the header was already read",
            ));
        }
        let (line_number, raw) = match self.read_raw_line()? {
            Some(line) => line,
            None => return Err(TraceError::EmptyTrace),
        };
        let line = match parse_raw_line(&raw, line_number) {
            Ok(line) => line,
            Err(err) => return Err(self.poison(err)),
        };
        match line.kind.as_str() {
            "header" => {
                let header = match self.parse_header(line.rest, line_number) {
                    Ok(header) => header,
                    Err(err) => return Err(self.poison(err)),
                };
                self.state = ReaderState::ReadingEvents;
                Ok(header)
            }
            other => Err(self.poison(TraceError::MissingHeader {
                kind: other.to_string(),
            })),
        }
    }

    /// Reads the next event line; returns `Ok(None)` at end of file.
    ///
    /// Enforces the non-decreasing time policy and field ranges. Must be
    /// called after [`TraceReader::read_header`]. A corrupted line, a time
    /// regression, or an I/O failure is terminal: the reader enters its
    /// failed state and later events are never consumed (see the module
    /// docs).
    pub fn read_event(&mut self) -> Result<Option<TraceEvent>, TraceError> {
        match self.state {
            ReaderState::Failed => {
                return Err(TraceError::Poisoned(
                    "read_event on a failed reader: a previous line failed to parse/validate",
                ));
            }
            ReaderState::AwaitingHeader => {
                return Err(TraceError::InvalidState(
                    "read_event called before read_header",
                ));
            }
            ReaderState::Finished => return Ok(None),
            ReaderState::ReadingEvents => {}
        }
        let Some((line_number, raw)) = self.read_raw_line()? else {
            self.state = ReaderState::Finished;
            return Ok(None);
        };
        let line = match parse_raw_line(&raw, line_number) {
            Ok(line) => line,
            Err(err) => return Err(self.poison(err)),
        };
        let event = match line.kind.as_str() {
            "event" => match self.parse_event(line.rest, line_number) {
                Ok(event) => event,
                Err(err) => return Err(self.poison(err)),
            },
            "header" => {
                return Err(self.poison(TraceError::DuplicateHeader { line_number }));
            }
            other => {
                return Err(self.poison(TraceError::UnknownLineKind {
                    line_number,
                    kind: other.to_string(),
                }));
            }
        };
        if let Some(previous) = self.last_time {
            let current = event.time();
            if current < previous {
                return Err(self.poison(TraceError::TimeRegression {
                    line_number,
                    previous,
                    current,
                }));
            }
        }
        self.last_time = Some(event.time());
        Ok(Some(event))
    }

    /// Marks the reader failed after a line-consuming error and returns the
    /// error itself. The reader is terminal from this point on.
    fn poison(&mut self, err: TraceError) -> TraceError {
        self.state = ReaderState::Failed;
        err
    }

    /// Iterates over the remaining events after the header has been read.
    ///
    /// Stops at the first error or at end of file. Convenience over
    /// [`TraceReader::read_event`].
    pub fn events(&mut self) -> Events<'_, R> {
        Events {
            reader: self,
            done: false,
        }
    }

    /// Reads one raw line, stripping the trailing newline. Returns `None` at
    /// end of file. An underlying I/O failure poisons the reader (it is
    /// terminal, per the module docs).
    fn read_raw_line(&mut self) -> Result<Option<(u64, String)>, TraceError> {
        let mut buffer = String::new();
        let read = self
            .inner
            .read_line(&mut buffer)
            .map_err(|err| self.poison(TraceError::Io(err)))?;
        if read == 0 {
            return Ok(None);
        }
        let line_number = self.line_number;
        self.line_number += 1;
        Ok(Some((line_number, buffer)))
    }

    /// Parses the header `kind` payload, validating schema version and clock.
    ///
    /// `schema_version` arrives as a raw [`serde_json::Number`] and is
    /// classified explicitly (module docs): negative or fractional values are
    /// `InvalidField`, a positive integral version newer than supported —
    /// including values above `i64::MAX` — is `SchemaTooNew`, and a
    /// non-number value is rejected earlier as `CorruptedLine` by the serde
    /// deserialization of [`RawHeader`].
    fn parse_header(
        &self,
        rest: Map<String, Value>,
        line_number: u64,
    ) -> Result<TraceHeader, TraceError> {
        let raw: RawHeader = serde_json::from_value(Value::Object(rest)).map_err(|source| {
            TraceError::CorruptedLine {
                line_number,
                message: source.to_string(),
            }
        })?;
        let found =
            parse_bounded_integer(&raw.schema_version, "schema_version", 0, u64::MAX as u128)
                .map_err(|message| TraceError::InvalidField {
                    line_number,
                    message,
                })? as u64;
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
        let clock = match raw.clock.as_str() {
            "monotonic" => TraceClock::Monotonic,
            other => {
                return Err(TraceError::InvalidField {
                    line_number,
                    message: format!(
                        "unsupported clock {other:?}; schema version 1 defines only \"monotonic\""
                    ),
                });
            }
        };
        Ok(TraceHeader {
            schema_version: SUPPORTED_SCHEMA_VERSION,
            clock,
            device: raw.device,
        })
    }

    /// Parses an event `kind` payload, validating field ranges and the
    /// convertibility of its timestamp.
    ///
    /// Every integer field arrives as a raw [`serde_json::Number`] and is
    /// classified explicitly (module docs): non-numbers are rejected earlier
    /// as `CorruptedLine` by the serde deserialization of [`RawEvent`], while
    /// fractional, negative, or out-of-range numbers are `InvalidField` here.
    /// This keeps values like `sec == i64::MAX + 1` (valid `u64` syntax,
    /// overflowing timestamp) classified as `InvalidField`, never as a
    /// corrupted line.
    fn parse_event(
        &self,
        rest: Map<String, Value>,
        line_number: u64,
    ) -> Result<TraceEvent, TraceError> {
        let raw: RawEvent = serde_json::from_value(Value::Object(rest)).map_err(|source| {
            TraceError::CorruptedLine {
                line_number,
                message: source.to_string(),
            }
        })?;
        let sec =
            parse_bounded_integer(&raw.sec, "sec", 0, u64::MAX as u128).map_err(|message| {
                TraceError::InvalidField {
                    line_number,
                    message,
                }
            })? as u64;
        let usec = parse_bounded_integer(&raw.usec, "usec", 0, u128::from(USEC_PER_SEC - 1))
            .map_err(|message| TraceError::InvalidField {
                line_number,
                message,
            })? as u32;
        let event_type = parse_bounded_integer(&raw.event_type, "type", 0, u16::MAX as u128)
            .map_err(|message| TraceError::InvalidField {
                line_number,
                message,
            })? as u16;
        let code =
            parse_bounded_integer(&raw.code, "code", 0, u16::MAX as u128).map_err(|message| {
                TraceError::InvalidField {
                    line_number,
                    message,
                }
            })? as u16;
        let value = parse_bounded_integer(&raw.value, "value", i32::MIN as i128, i32::MAX as u128)
            .map_err(|message| TraceError::InvalidField {
            line_number,
            message,
        })? as i32;
        let event = TraceEvent {
            sec,
            usec,
            event_type,
            code,
            value,
        };
        event
            .validate_fields()
            .map_err(|err| TraceError::InvalidField {
                line_number,
                message: err.to_string(),
            })?;
        Ok(event)
    }
}

/// Iterator over the events of a trace after its header.
///
/// Yields `Ok(event)` for each event and stops at the first error (yielding
/// it once) or at end of file.
pub struct Events<'a, R: Read> {
    reader: &'a mut TraceReader<R>,
    done: bool,
}

impl<R: Read> Iterator for Events<'_, R> {
    type Item = Result<TraceEvent, TraceError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.reader.read_event() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(err) => {
                self.done = true;
                Some(Err(err))
            }
        }
    }
}

/// A raw JSON line: the `kind` discriminator plus every other field, so the
/// reader can classify lines itself (unknown kinds get a dedicated error
/// rather than a generic parse failure).
#[derive(Deserialize)]
struct RawLine {
    kind: String,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// Parses one line into a [`RawLine`], classifying malformed lines as
/// corrupted.
fn parse_raw_line(line: &str, line_number: u64) -> Result<RawLine, TraceError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(TraceError::CorruptedLine {
            line_number,
            message: "line is empty".to_string(),
        });
    }
    serde_json::from_str(trimmed).map_err(|source| TraceError::CorruptedLine {
        line_number,
        message: source.to_string(),
    })
}

/// Raw header fields. Integer fields arrive as raw [`serde_json::Number`]s
/// so the reader can classify out-of-range values itself instead of letting
/// serde reject them as generic parse errors (module docs). The device
/// description is a nested `device` object deserialized into a
/// [`DeviceDescriptor`] (unknown extra fields inside are ignored, per the
/// forward-compatibility policy). A missing field or a non-number
/// `schema_version` fails this deserialization and is reported as a
/// corrupted line.
#[derive(Deserialize)]
struct RawHeader {
    schema_version: serde_json::Number,
    clock: String,
    device: DeviceDescriptor,
}

/// Raw event fields, parsed as raw [`serde_json::Number`]s so negative,
/// fractional, or out-of-range values become precise invalid-field
/// diagnostics (module docs) instead of generic serde parse errors.
#[derive(Deserialize)]
struct RawEvent {
    sec: serde_json::Number,
    usec: serde_json::Number,
    #[serde(rename = "type")]
    event_type: serde_json::Number,
    code: serde_json::Number,
    value: serde_json::Number,
}

/// Classifies a raw JSON number as an integer in `min..=max`, returning the
/// value as an `i128` (wide enough for every field in the schema).
///
/// This is the explicit signedness/integrality/range check the reader applies
/// to every integer field:
///
/// * a number stored as a float (fractional or exponent form) is not an
///   integer — an error message, never a silently truncated value;
/// * an integer below `min` or above `max` (which covers negative values for
///   unsigned fields) is out of range;
/// * anything else converts losslessly.
///
/// Non-number JSON values never reach this function: they fail the `Number`
/// deserialization of [`RawHeader`]/[`RawEvent`] and are reported as
/// corrupted lines.
fn parse_bounded_integer(
    number: &serde_json::Number,
    field: &str,
    min: i128,
    max: u128,
) -> Result<i128, String> {
    if let Some(value) = number.as_u64() {
        let value = u128::from(value);
        if value > max {
            return Err(format!("{field} must be in [{min}, {max}], found {value}"));
        }
        // `value <= max <= u64::MAX`, which fits in i128.
        return Ok(value as i128);
    }
    if let Some(value) = number.as_i64() {
        let value = i128::from(value);
        if value < min {
            return Err(format!("{field} must be in [{min}, {max}], found {value}"));
        }
        return Ok(value);
    }
    if let Some(value) = number.as_f64() {
        return Err(format!(
            "{field} must be an integer written without a fractional part or exponent, found {value}"
        ));
    }
    Err(format!("{field} must be a JSON number"))
}

const USEC_PER_SEC: u64 = crate::time::USEC_PER_SEC;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn reader_from(text: &str) -> TraceReader<Cursor<&[u8]>> {
        TraceReader::new(Cursor::new(text.as_bytes()))
    }

    fn event(sec: u64, usec: u32, event_type: u16, code: u16, value: i32) -> TraceEvent {
        TraceEvent::new(sec, usec, event_type, code, value)
    }

    const VALID_HEADER: &str = r#"{"kind":"header","schema_version":1,"clock":"monotonic","device":{"name":"dev","vendor_id":0,"product_id":0,"axes":{},"slot_count":10,"supports_type_b_mt":true,"has_physical_buttons":true,"profile":{"name":"default","axis_resolutions":{},"quirks":[]}}}"#;

    fn header_line() -> String {
        VALID_HEADER.to_string()
    }

    fn single_event_line() -> String {
        r#"{"kind":"event","sec":0,"usec":1000,"type":3,"code":47,"value":0}"#.to_string()
    }

    #[test]
    fn reads_header_then_events() {
        let text = format!(
            "{}\n{}\n{}",
            header_line(),
            single_event_line(),
            single_event_line()
        );
        let mut reader = reader_from(&text);
        let header = reader.read_header().unwrap();
        assert_eq!(header.schema_version, 1);
        assert_eq!(header.clock, TraceClock::Monotonic);
        assert_eq!(header.device.name, "dev");
        let first = reader.read_event().unwrap().unwrap();
        assert_eq!(first, event(0, 1000, 3, 47, 0));
        assert!(reader.read_event().unwrap().is_some());
        assert!(reader.read_event().unwrap().is_none());
    }

    #[test]
    fn header_only_trace_is_valid_with_zero_events() {
        let header = header_line();
        let mut reader = reader_from(&header);
        reader.read_header().unwrap();
        assert!(reader.read_event().unwrap().is_none());
        // EOF is sticky.
        assert!(reader.read_event().unwrap().is_none());
    }

    #[test]
    fn empty_file_reports_empty_trace() {
        let err = reader_from("").read_header().unwrap_err();
        assert!(matches!(err, TraceError::EmptyTrace));
    }

    #[test]
    fn first_line_must_be_header() {
        let text = format!("{}\n{}", single_event_line(), header_line());
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(err, TraceError::MissingHeader { ref kind } if kind == "event"));
    }

    #[test]
    fn whitespace_only_first_line_is_corrupted_not_header() {
        let err = reader_from("\n\n").read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 1, .. }
        ));
    }

    #[test]
    fn duplicate_header_is_rejected() {
        let text = format!("{}\n{}", header_line(), header_line());
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::DuplicateHeader { line_number: 2 }
        ));
    }

    #[test]
    fn unknown_line_kind_is_rejected() {
        let text = format!("{}\n{}", header_line(), r#"{"kind":"calibration","x":1}"#);
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(
            matches!(err, TraceError::UnknownLineKind { line_number: 2, ref kind } if kind == "calibration")
        );
    }

    #[test]
    fn unknown_optional_fields_are_ignored_for_forward_compat() {
        // Extra unknown fields on both header and event lines must not fail,
        // and unknown fields inside the nested device object must not fail
        // either.
        let header_with_extra = r#"{"kind":"header","schema_version":1,"clock":"monotonic","device":{"name":"dev","vendor_id":0,"product_id":0,"axes":{},"slot_count":10,"supports_type_b_mt":true,"has_physical_buttons":true,"profile":{"name":"default","axis_resolutions":{},"quirks":[]},"future_device_field":42},"future_header_field":"ignored"}"#;
        let event_with_extra = r#"{"kind":"event","sec":0,"usec":1000,"type":3,"code":47,"value":0,"future_event_field":{"nested":true}}"#;
        let text = format!("{header_with_extra}\n{event_with_extra}");
        let mut reader = reader_from(&text);
        let header = reader.read_header().unwrap();
        assert_eq!(header.schema_version, 1);
        let ev = reader.read_event().unwrap().unwrap();
        assert_eq!(ev, event(0, 1000, 3, 47, 0));
    }

    #[test]
    fn schema_too_new_fails_explicitly() {
        let text = header_line().replace("\"schema_version\":1", "\"schema_version\":2");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::SchemaTooNew {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn schema_too_old_fails_explicitly() {
        let text = header_line().replace("\"schema_version\":1", "\"schema_version\":0");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::SchemaTooOld {
                found: 0,
                supported: 1
            }
        ));
    }

    #[test]
    fn negative_schema_version_is_invalid_field() {
        let text = header_line().replace("\"schema_version\":1", "\"schema_version\":-3");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 1, .. }
        ));
    }

    #[test]
    fn unsupported_clock_is_invalid_field() {
        let text = header_line().replace("\"clock\":\"monotonic\"", "\"clock\":\"realtime\"");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(
            matches!(err, TraceError::InvalidField { line_number: 1, ref message } if message.contains("realtime"))
        );
    }

    #[test]
    fn missing_schema_version_is_corrupted_line() {
        let text = header_line().replace("\"schema_version\":1,", "");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 1, .. }
        ));
    }

    #[test]
    fn malformed_json_is_corrupted_line() {
        let text = format!("{}\n{{{{not json", header_line());
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 2, .. }
        ));
    }

    #[test]
    fn missing_event_field_is_corrupted_line() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":1000,"type":3,"code":47}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 2, .. }
        ));
    }

    #[test]
    fn out_of_range_usec_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":1000000,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(
            matches!(err, TraceError::InvalidField { line_number: 2, ref message } if message.contains("usec"))
        );
    }

    #[test]
    fn negative_usec_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":-1,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(
            matches!(err, TraceError::InvalidField { line_number: 2, ref message } if message.contains("usec"))
        );
    }

    #[test]
    fn negative_sec_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":-1,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(
            matches!(err, TraceError::InvalidField { line_number: 2, ref message } if message.contains("sec"))
        );
    }

    #[test]
    fn time_regression_is_rejected() {
        let text = format!(
            "{}\n{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":2000,"type":3,"code":47,"value":0}"#,
            r#"{"kind":"event","sec":0,"usec":1000,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        reader.read_event().unwrap().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::TimeRegression { line_number: 3, previous, current }
                if previous == TraceTime { sec: 0, usec: 2000 }
                    && current == TraceTime { sec: 0, usec: 1000 }
        ));
    }

    #[test]
    fn equal_times_are_accepted() {
        let text = format!(
            "{}\n{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":2000,"type":3,"code":47,"value":0}"#,
            r#"{"kind":"event","sec":0,"usec":2000,"type":3,"code":53,"value":1}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        reader.read_event().unwrap().unwrap();
        assert!(reader.read_event().unwrap().is_some());
    }

    #[test]
    fn unconvertible_timestamp_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":18446744073709,"usec":516,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(
            matches!(err, TraceError::InvalidField { line_number: 2, ref message } if message.contains("nanoseconds"))
        );
    }

    #[test]
    fn read_event_before_read_header_is_invalid_state() {
        let header = header_line();
        let mut reader = reader_from(&header);
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::InvalidState(_)));
    }

    #[test]
    fn read_header_twice_is_invalid_state() {
        let header = header_line();
        let mut reader = reader_from(&header);
        reader.read_header().unwrap();
        let err = reader.read_header().unwrap_err();
        assert!(matches!(err, TraceError::InvalidState(_)));
    }

    #[test]
    fn events_iterator_streams() {
        let text = format!(
            "{}\n{}\n{}",
            header_line(),
            single_event_line(),
            single_event_line()
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let events: Vec<_> = reader.events().map(Result::unwrap).collect();
        assert_eq!(events.len(), 2);
        assert!(reader.read_event().unwrap().is_none());
    }

    #[test]
    fn events_iterator_stops_at_error() {
        let text = format!(
            "{}\n{}\n{}\n{}",
            header_line(),
            single_event_line(),
            r#"{"kind":"event","sec":0,"usec":999,"type":3,"code":47,"value":0}"#,
            single_event_line()
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let mut iter = reader.events();
        assert!(iter.next().unwrap().is_ok());
        assert!(matches!(
            iter.next(),
            Some(Err(TraceError::TimeRegression { .. }))
        ));
        assert!(iter.next().is_none());
    }

    // --- Numeric field classification (R2) ---

    #[test]
    fn schema_version_above_i64_max_is_schema_too_new() {
        // 2^63 is a positive integral version representable in u64, so it
        // must produce the promised SchemaTooNew result, not a parse error.
        let text = header_line().replace(
            "\"schema_version\":1",
            "\"schema_version\":9223372036854775808",
        );
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::SchemaTooNew {
                found: 9_223_372_036_854_775_808,
                supported: 1
            }
        ));
    }

    #[test]
    fn non_number_schema_version_is_corrupted_line() {
        // A present but non-numeric schema_version is wrong-shaped JSON.
        let text = header_line().replace("\"schema_version\":1", "\"schema_version\":\"1\"");
        let err = reader_from(&text).read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 1, .. }
        ));
    }

    #[test]
    fn sec_above_i64_max_is_invalid_field_not_corrupted() {
        // 2^63 is syntactically a valid u64, so it must be classified by
        // field validation (timestamp overflow -> InvalidField), never as a
        // corrupted line.
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":9223372036854775808,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("nanoseconds")
        ));
    }

    #[test]
    fn sec_at_u64_max_is_invalid_field_not_corrupted() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":18446744073709551615,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("nanoseconds")
        ));
    }

    #[test]
    fn negative_type_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":-1,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("type")
        ));
    }

    #[test]
    fn overflowing_type_is_invalid_field() {
        // 65536 is one past u16::MAX.
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":65536,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("type")
        ));
    }

    #[test]
    fn negative_code_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":3,"code":-1,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("code")
        ));
    }

    #[test]
    fn overflowing_code_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":3,"code":65536,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("code")
        ));
    }

    #[test]
    fn overflowing_value_is_invalid_field() {
        // i32::MAX + 1.
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":3,"code":47,"value":2147483648}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("value")
        ));
    }

    #[test]
    fn value_below_i32_min_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":0,"type":3,"code":47,"value":-2147483649}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("value")
        ));
    }

    #[test]
    fn fractional_numeric_field_is_invalid_field() {
        // A present numeric field that is not an integer (fractional form)
        // is classified as an invalid field, never silently truncated.
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":1.5,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("integer")
        ));
    }

    #[test]
    fn exponent_form_numeric_field_is_invalid_field() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":1e3,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::InvalidField { line_number: 2, ref message } if message.contains("integer")
        ));
    }

    #[test]
    fn non_number_field_is_corrupted_line() {
        // A present but non-numeric value for an integer field is
        // wrong-shaped JSON: corrupted line, not an invalid field.
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":"abc","usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 2, .. }
        ));
    }

    #[test]
    fn boolean_field_is_corrupted_line() {
        let text = format!(
            "{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":true,"usec":0,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 2, .. }
        ));
    }

    // --- Terminal failure contract (R3) ---

    #[test]
    fn failed_header_poisons_reader_and_second_line_header_is_rejected() {
        // The first line is an event, not the header: `MissingHeader`. The
        // reader must not accept the header on line 2 afterwards.
        let text = format!("{}\n{}", single_event_line(), header_line());
        let mut reader = reader_from(&text);
        let err = reader.read_header().unwrap_err();
        assert!(matches!(
            err,
            TraceError::MissingHeader { ref kind } if kind == "event"
        ));
        let err = reader.read_header().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
    }

    #[test]
    fn corrupted_line_poisons_reader() {
        let text = format!(
            "{}\n{}\n{}",
            header_line(),
            "{{{{not json",
            single_event_line()
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::CorruptedLine { line_number: 2, .. }
        ));
        // The later, valid event on line 3 must never be consumed.
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
    }

    #[test]
    fn time_regression_poisons_reader() {
        let text = format!(
            "{}\n{}\n{}\n{}",
            header_line(),
            r#"{"kind":"event","sec":0,"usec":2000,"type":3,"code":47,"value":0}"#,
            r#"{"kind":"event","sec":0,"usec":1000,"type":3,"code":47,"value":0}"#,
            r#"{"kind":"event","sec":0,"usec":3000,"type":3,"code":47,"value":0}"#
        );
        let mut reader = reader_from(&text);
        reader.read_header().unwrap();
        reader.read_event().unwrap().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(
            err,
            TraceError::TimeRegression { line_number: 3, .. }
        ));
        // The later event on line 4 must never be consumed.
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
    }

    /// A `Read` that yields `data` once and then fails instead of returning
    /// end of file — proves underlying I/O failures poison the reader.
    struct FailAfterRead {
        data: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for FailAfterRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Err(std::io::Error::other("injected read failure"));
            }
            let n = buf.len().min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn io_error_during_header_poisons_reader() {
        let mut reader = TraceReader::new(FailAfterRead {
            data: Vec::new(),
            pos: 0,
        });
        let err = reader.read_header().unwrap_err();
        assert!(matches!(err, TraceError::Io(_)));
        let err = reader.read_header().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
    }

    #[test]
    fn io_error_during_events_poisons_reader() {
        let text = format!("{}\n", header_line());
        let mut reader = TraceReader::new(FailAfterRead {
            data: text.into_bytes(),
            pos: 0,
        });
        reader.read_header().unwrap();
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Io(_)));
        let err = reader.read_event().unwrap_err();
        assert!(matches!(err, TraceError::Poisoned(_)));
    }

    #[test]
    fn api_misuse_does_not_poison_the_reader() {
        // API-misuse errors consume no line, so they must not poison: the
        // reader keeps working after an InvalidState.
        let text = format!("{}\n{}", header_line(), single_event_line());
        let mut reader = reader_from(&text);
        assert!(matches!(
            reader.read_event(),
            Err(TraceError::InvalidState(_))
        ));
        let header = reader.read_header().unwrap();
        assert_eq!(header.schema_version, 1);
        assert!(reader.read_event().unwrap().is_some());
        assert!(reader.read_event().unwrap().is_none());
    }
}
