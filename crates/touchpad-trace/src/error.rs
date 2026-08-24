//! Structured errors for trace reading, writing, and replay.

use crate::time::TraceTime;

/// Failure modes of trace reading, writing, and replay.
///
/// The taxonomy (IMPLEMENTATION_BRIEF §3.3) deliberately distinguishes:
///
/// * **unsupported schema versions** — [`TraceError::SchemaTooNew`] /
///   [`TraceError::SchemaTooOld`];
/// * **corrupted lines** — [`TraceError::CorruptedLine`]: not valid JSON or
///   not shaped like a trace line;
/// * **invalid fields** — [`TraceError::InvalidField`]: well-formed JSON with
///   an out-of-range or unsupported value;
/// * **header problems** — [`TraceError::EmptyTrace`] /
///   [`TraceError::MissingHeader`] / [`TraceError::DuplicateHeader`];
/// * **time policy violations** — [`TraceError::TimeRegression`];
/// * **I/O errors** — [`TraceError::Io`];
/// * **poisoned streams** — [`TraceError::Poisoned`]: after a failure that
///   consumed/partially-wrote a line (including underlying I/O failure), the
///   stream is marked terminal and every further operation is rejected, so a
///   caller can never resume as if the offending line did not happen;
/// * **API misuse** — [`TraceError::InvalidState`].
#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    /// An I/O failure on the underlying reader or writer.
    #[error("trace I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A line is not valid JSON or does not have the shape of a trace line.
    /// `line_number` is the 1-based line in the trace file.
    #[error("trace line {line_number} is corrupted: {message}")]
    CorruptedLine {
        /// 1-based line number in the trace file.
        line_number: u64,
        /// Human-readable reason (includes the JSON parser message).
        message: String,
    },

    /// The trace declares a `schema_version` newer than this build supports.
    #[error(
        "trace declares schema version {found}, newer than this build supports (max {supported})"
    )]
    SchemaTooNew {
        /// Version declared by the trace.
        found: u64,
        /// Highest version this build can read.
        supported: u32,
    },

    /// The trace declares a `schema_version` older than any this build knows.
    #[error("trace declares schema version {found}, older than the supported version {supported}")]
    SchemaTooOld {
        /// Version declared by the trace.
        found: u64,
        /// The oldest version this build knows (always 1).
        supported: u32,
    },

    /// The trace file is empty; the mandatory header line is missing.
    #[error("trace is empty: the mandatory header line is missing")]
    EmptyTrace,

    /// The first line is not a header, so the trace has no header.
    #[error("the first line of a trace must be the header, but line 1 has kind {kind:?}")]
    MissingHeader {
        /// The `kind` of the first line.
        kind: String,
    },

    /// A header appeared again after the first line.
    #[error(
        "the header must be the first line and appear exactly once; found another header on line {line_number}"
    )]
    DuplicateHeader {
        /// 1-based line number of the duplicate header.
        line_number: u64,
    },

    /// A line has a `kind` this reader cannot interpret.
    ///
    /// Unknown optional *fields* are ignored (forward compatibility), but an
    /// unknown *line kind* means the line carries semantics this reader
    /// cannot reproduce — skipping it could silently mis-replay, so it is an
    /// error.
    #[error(
        "line {line_number} has unknown kind {kind:?}; unknown line kinds are rejected — only unknown optional fields are ignored for forward compatibility"
    )]
    UnknownLineKind {
        /// 1-based line number of the offending line.
        line_number: u64,
        /// The unknown `kind` value.
        kind: String,
    },

    /// A line is well-formed JSON but carries an out-of-range or otherwise
    /// unsupported field value (e.g. `usec` outside `[0, 999_999]`, a
    /// negative `sec`, or an unsupported clock).
    #[error("line {line_number} has an invalid field: {message}")]
    InvalidField {
        /// 1-based line number of the offending line.
        line_number: u64,
        /// Why the field is invalid.
        message: String,
    },

    /// Event timestamps must be non-decreasing; this event goes backwards.
    #[error(
        "event time went backwards on line {line_number}: previous {previous:?} -> current {current:?}"
    )]
    TimeRegression {
        /// 1-based line number of the offending event.
        line_number: u64,
        /// The previous event's time.
        previous: TraceTime,
        /// The current (regressed) event's time.
        current: TraceTime,
    },

    /// A reader/writer method was called in an order the API does not allow
    /// (e.g. `read_event` before `read_header`, or writing after the writer
    /// was finished).
    #[error("invalid reader/writer state: {0}")]
    InvalidState(&'static str),

    /// The stream hit a failure after which its state can no longer be
    /// trusted, and it is now terminal.
    ///
    /// * A **writer** enters this state when writing an event line fails with
    ///   an I/O error: `Write::write_all` may have written a *prefix* of the
    ///   line before failing, so a retry would append a new JSON object to
    ///   that prefix and silently corrupt the trace. After poisoning,
    ///   [`crate::TraceWriter::write_event`], [`crate::TraceWriter::flush`]
    ///   and [`crate::TraceWriter::finish`] are all rejected.
    /// * A **reader** enters this state whenever consuming/parsing/validating
    ///   a trace line fails (including underlying I/O failure), so the
    ///   offending line can never be skipped: a second-line header is not
    ///   accepted after line 1 failed, and events after a corrupted or
    ///   regressed line are never consumed. The original failure is reported
    ///   once; every subsequent header/event operation returns this error.
    #[error("trace stream is poisoned after a partial failure: {0}")]
    Poisoned(&'static str),

    /// A trace line could not be serialized. This indicates a bug in the
    /// crate (the schema types are always serializable), surfaced rather
    /// than panicking.
    #[error("trace line could not be serialized: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_displayable_and_source_carrying() {
        let err = TraceError::TimeRegression {
            line_number: 7,
            previous: TraceTime { sec: 0, usec: 1000 },
            current: TraceTime { sec: 0, usec: 900 },
        };
        let message = err.to_string();
        assert!(message.contains("line 7"));
        assert!(message.contains("backwards"));

        let io = TraceError::Io(std::io::Error::other("boom"));
        assert!(io.to_string().contains("boom"));
    }
}
