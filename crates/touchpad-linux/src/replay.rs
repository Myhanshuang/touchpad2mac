//! [`ReplaySink`] implementation for the Type-B decoder.
//!
//! This is how offline replay drives the **exact same** decoder state machine
//! as live input: [`touchpad_trace::ReplayDriver`] delivers the trace header to
//! [`ReplaySink::on_header`] (which configures the decoder with the trace's
//! device descriptor — the same device model live input uses) and each raw
//! event to [`ReplaySink::on_event`] (which converts it to [`RawEvent`] and
//! feeds the decoder). No second decoder or second state model exists.
//!
//! ## Finish semantics
//!
//! [`ReplaySink::finish`] distinguishes two ways a trace can end:
//!
//! * **Ending between frames is fine.** The decoder only publishes frames at
//!   `SYN_REPORT` boundaries, so a trace whose last event leaves the decoder
//!   in [`SyncState::Normal`] is a clean, complete replay.
//! * **Ending with unresolved synchronization loss is an error.** A trace
//!   that ends after `SYN_DROPPED` but before the recovery `SYN_REPORT`
//!   leaves the decoder in `DroppedAwaitingBoundary` (or `Recovering` /
//!   `Degraded`): continuity was never restored, so `finish` returns
//!   [`ReplayDecodeError::UnresolvedSynchronizationLoss`] and emits no frame.
#![forbid(unsafe_code)]

use touchpad_trace::{ReplaySink, TraceClock, TraceEvent, TraceHeader, SUPPORTED_SCHEMA_VERSION};

use crate::decode::{DecodeError, SyncState, TypeBDecoder};
use crate::rawevent::RawEvent;
use crate::sink::FrameSink;

/// Failure of the decoder when driven by `ReplayDriver`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReplayDecodeError {
    /// The decoder rejected or failed on the input (including a fatal
    /// `SYN_DROPPED` resynchronization failure).
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The trace declares a schema version the decoder does not support.
    #[error(
        "replayed trace declares schema version {0}; only version {SUPPORTED_SCHEMA_VERSION} is supported"
    )]
    UnsupportedSchema(u32),
    /// The trace uses a clock other than monotonic.
    #[error("replayed trace uses a non-monotonic clock")]
    UnsupportedClock,
    /// A trace timestamp cannot be represented as a
    /// [`touchpad_core::Monotonic`].
    #[error("replayed event timestamp cannot be represented as a monotonic timestamp")]
    UnrepresentableTimestamp,
    /// The trace ended while the decoder had not restored synchronization
    /// after `SYN_DROPPED`; the replay must not report clean completion.
    #[error(
        "trace ended while the decoder was in sync state {0:?}: the stream lost synchronization (SYN_DROPPED) and it was never restored"
    )]
    UnresolvedSynchronizationLoss(SyncState),
}

impl<S: FrameSink> ReplaySink for TypeBDecoder<S> {
    type Error = ReplayDecodeError;

    fn on_header(&mut self, header: &TraceHeader) -> Result<(), Self::Error> {
        if header.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ReplayDecodeError::UnsupportedSchema(header.schema_version));
        }
        if header.clock != TraceClock::Monotonic {
            return Err(ReplayDecodeError::UnsupportedClock);
        }
        self.configure(header.device.clone())?;
        Ok(())
    }

    fn on_event(&mut self, event: &TraceEvent) -> Result<(), Self::Error> {
        let raw =
            RawEvent::from_trace_event(event).ok_or(ReplayDecodeError::UnrepresentableTimestamp)?;
        self.feed(raw)?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        // Only a trustworthy terminal synchronization state completes a
        // replay. Ending between frames (Normal) is fine; ending with
        // unresolved loss of synchronization (DroppedAwaitingBoundary,
        // Recovering, or Degraded) is not, and emits no frame (M3 review R5).
        match self.sync_state() {
            SyncState::Normal => Ok(()),
            state => Err(ReplayDecodeError::UnresolvedSynchronizationLoss(state)),
        }
    }
}
