//! The decoder's raw input boundary ([`RawEvent`]).
//!
//! [`RawEvent`] is the single input representation the Type-B decoder
//! accepts. Live input (M4) will build it from kernel `struct input_event`
//! values; the offline replay path converts [`TraceEvent`]s into it
//! ([`RawEvent::from_trace_event`]). Because both paths feed the exact same
//! [`crate::decode::TypeBDecoder`], replay exercises the same state machine
//! as live input — there is no second decoder.
#![forbid(unsafe_code)]

use touchpad_core::Monotonic;
use touchpad_trace::TraceEvent;

/// One raw kernel-style input event at the decoder boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawEvent {
    /// Monotonic timestamp supplied by the input edge (kernel
    /// `CLOCK_MONOTONIC` for live input, the trace clock for replay).
    pub timestamp: Monotonic,
    /// Kernel event type (`EV_SYN`, `EV_KEY`, `EV_ABS`, ...).
    pub event_type: u16,
    /// Kernel event code (`SYN_REPORT`, `ABS_MT_*`, `BTN_*`, ...).
    pub code: u16,
    /// Kernel event value (signed 32-bit).
    pub value: i32,
}

impl RawEvent {
    /// Creates a raw event.
    #[must_use]
    pub const fn new(timestamp: Monotonic, event_type: u16, code: u16, value: i32) -> Self {
        Self {
            timestamp,
            event_type,
            code,
            value,
        }
    }

    /// Converts a trace event into the decoder's raw boundary.
    ///
    /// This is the only path from a trace into the decoder; the timestamp is
    /// converted from the trace's `(sec, usec)` pair via
    /// [`TraceTime::to_monotonic`](touchpad_trace::TraceTime::to_monotonic).
    /// Returns `None` when the timestamp cannot be represented — the trace
    /// reader already rejects such events, so this is defensive.
    #[must_use]
    pub fn from_trace_event(event: &TraceEvent) -> Option<Self> {
        Some(Self {
            timestamp: event.time().to_monotonic()?,
            event_type: event.event_type,
            code: event.code,
            value: event.value,
        })
    }
}
