//! # touchpad-trace
//!
//! Versioned **raw input trace** reader/writer and the platform-neutral
//! **offline replay boundary** (M2 — Versioned Trace and Offline Replay
//! Boundary).
//!
//! A raw trace is a JSON Lines file that captures exactly what a physical
//! input device produced, **before any decoding** (design.md §15,
//! IMPLEMENTATION_BRIEF §3.3 and §8). It is the regression-test artifact of
//! the runtime: the recorder sits in front of the decoder, so even a decoder
//! bug leaves the original input available for reproduction.
//!
//! ## File format (schema version 1)
//!
//! One JSON object per line. The **first line must be the header and the
//! header must appear exactly once**:
//!
//! ```json
//! {"kind":"header","schema_version":1,"clock":"monotonic","device":{"name":"...","vendor_id":0,"product_id":0,"axes":{...},"slot_count":10,"supports_type_b_mt":true,"has_physical_buttons":true,"profile":{...}}}
//! ```
//!
//! Every following line is a raw kernel-style input event:
//!
//! ```json
//! {"kind":"event","sec":0,"usec":1234,"type":3,"code":47,"value":0}
//! ```
//!
//! ## Contract highlights
//!
//! * **Streaming.** The reader processes one line at a time and never loads a
//!   whole trace into memory; large traces are handled with constant memory
//!   (one line). The writer emits one line at a time.
//! * **No wall clock.** The header declares the clock domain
//!   ([`TraceClock::Monotonic`]); event times are `(sec, usec)` pairs that
//!   convert to the core [`Monotonic`] type (see [`TraceTime::to_monotonic`],
//!   a checked conversion). The trace crate never reads a wall clock and
//!   replay never paces events by wall time.
//! * **No real devices.** Reading, writing, and replaying a trace touch no
//!   `/dev/input` node, no ioctl, and no display server.
//! * **Versioning.** [`SUPPORTED_SCHEMA_VERSION`] is `1`. A trace declaring a
//!   newer schema fails with [`TraceError::SchemaTooNew`]; an older one with
//!   [`TraceError::SchemaTooOld`].
//! * **Forward compatibility.** Within schema version 1, the reader
//!   **ignores unknown optional fields** on header and event lines (serde's
//!   default: no `deny_unknown_fields`). Writers may add optional fields
//!   without bumping the version; anything structural (new line kinds, new
//!   required fields, new clock semantics) must bump `schema_version`.
//!   Unknown **line kinds** are rejected ([`TraceError::UnknownLineKind`]),
//!   never skipped: replay cannot reproduce semantics it does not
//!   understand, and silently skipping could mis-replay.
//! * **Time policy.** Field ranges: `sec` is a non-negative whole second,
//!   `usec` is in `[0, 999_999]`. The reader rejects out-of-range fields
//!   ([`TraceError::InvalidField`]) and requires **non-decreasing** event
//!   times ([`TraceError::TimeRegression`]), because replay timing semantics
//!   (timeouts, velocity) depend on a monotonic timeline. The writer records
//!   timestamps **faithfully**: it validates field ranges but deliberately
//!   does *not* reject time regressions, so a recorder never drops a
//!   real-but-odd kernel timestamp. Consequently **not every trace the
//!   writer produces is replay-accepted**: a regressed capture is a
//!   faithfully recorded but replay-invalid diagnostic artifact.
//! * **Error taxonomy.** [`TraceError`] distinguishes unsupported schema
//!   versions, corrupted lines (not valid JSON / not shaped like a trace
//!   line), invalid field values, missing/duplicate headers, time
//!   regressions, I/O errors, and **poisoned streams**: after a failure that
//!   consumed or partially wrote a line (including underlying I/O failure),
//!   the reader/writer becomes terminal ([`TraceError::Poisoned`]) and never
//!   resumes as if the offending line did not happen.
//! * **Numeric classification.** The reader parses integer fields as raw
//!   JSON numbers and classifies them explicitly: missing/non-number values
//!   are corrupted lines; fractional/negative/out-of-range numbers are
//!   invalid fields (so `sec` above `i64::MAX` and out-of-range `type` /
//!   `code` / `value` reach field validation, and a positive integral
//!   `schema_version` above `i64::MAX` yields `SchemaTooNew`).
//! * **Replay boundary.** [`replay::ReplaySink`] is the platform-neutral
//!   contract a raw-event consumer implements. The M3 Type-B decoder will
//!   implement it with the *same* state machine it uses for live input.
//!   **M2 deliberately ships no decoder and produces no `ContactFrame`
//!   output** — the boundary forwards raw events only; tests observe them
//!   with a recording sink.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::Serialize;

pub mod error;
pub mod event;
pub mod header;
pub mod reader;
pub mod replay;
pub mod time;
pub mod writer;

pub use error::TraceError;
pub use event::{TraceEvent, TraceFieldError};
pub use header::{TraceClock, TraceHeader};
pub use reader::{Events, TraceReader};
pub use replay::{RecordingSink, ReplayDriver, ReplayError, ReplaySink, ReplayStats, SinkError};
pub use time::TraceTime;
pub use writer::TraceWriter;

/// One line of a trace file, discriminated by its `kind` field.
///
/// [`TraceLine::Header`] serializes as `{"kind":"header",...}` and
/// [`TraceLine::Event`] as `{"kind":"event",...}`; the `kind` discriminator
/// is structural and stored in the line, not in the header/event structs
/// themselves. This type is the **writer's** line model (the writer always
/// emits the discriminator through it) and is serialize-only: the read path
/// is [`TraceReader`], which reports the full error taxonomy (unknown kinds,
/// invalid fields, time regression, ...) instead of raw serde errors.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TraceLine {
    /// The mandatory first line.
    Header(TraceHeader),
    /// A raw input event line.
    Event(TraceEvent),
}

/// The trace schema version this crate reads and writes.
///
/// Schema version 1 is the only supported version. Traces with a newer
/// `schema_version` are rejected with [`TraceError::SchemaTooNew`]; a
/// version older than 1 with [`TraceError::SchemaTooOld`].
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;
