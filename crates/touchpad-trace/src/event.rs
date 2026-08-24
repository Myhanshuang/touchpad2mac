//! Raw input events as recorded in a trace.
//!
//! An event line preserves exactly what the input device produced — the
//! kernel-style `(type, code, value)` triplet and its `(sec, usec)`
//! timestamp. The Type-B decoder (M3) consumes these to reconstruct frames;
//! a trace never stores decoded data.

use serde::{Deserialize, Serialize};

use crate::time::{TraceTime, USEC_PER_SEC};

/// Field-level validation failure of a [`TraceEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TraceFieldError {
    /// `usec` is outside `[0, 999_999]`.
    #[error("usec must be in [0, 999999], found {0}")]
    UsecOutOfRange(u32),
    /// `sec * 1_000_000_000 + usec * 1_000` does not fit in `u64`
    /// nanoseconds, so the timestamp cannot become a core `Monotonic`.
    #[error("timestamp ({sec}s, {usec}us) does not fit in u64 nanoseconds")]
    TimeOverflow {
        /// Whole seconds.
        sec: u64,
        /// Microseconds within the second.
        usec: u32,
    },
}

/// One raw input event in a trace.
///
/// Field ranges (schema version 1):
///
/// * `sec`: non-negative whole seconds (`u64`).
/// * `usec`: `[0, 999_999]` (`u32`).
/// * `event_type` / `code`: 16-bit kernel input event type/code (`u16`), the
///   `EV_*` / `ABS_*` / `KEY_*` codes of the Linux input protocol.
/// * `value`: 32-bit signed event value (`i32`).
///
/// The JSON shape is flat: `{"kind":"event","sec":0,"usec":1234,"type":3,
/// "code":47,"value":0}` — `"type"` is a JSON reserved word, so the Rust
/// field is named [`TraceEvent::event_type`] with `#[serde(rename = "type")]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Whole seconds of the event timestamp; non-negative.
    pub sec: u64,
    /// Microseconds within the second; `[0, 999_999]`.
    pub usec: u32,
    /// Kernel input event type (e.g. `EV_SYN == 0`, `EV_KEY == 1`,
    /// `EV_ABS == 3`).
    #[serde(rename = "type")]
    pub event_type: u16,
    /// Kernel input event code (e.g. `SYN_REPORT == 0`, `ABS_MT_SLOT == 47`).
    pub code: u16,
    /// Kernel input event value (signed 32-bit).
    pub value: i32,
}

impl TraceEvent {
    /// Creates a raw event.
    #[must_use]
    pub const fn new(sec: u64, usec: u32, event_type: u16, code: u16, value: i32) -> Self {
        Self {
            sec,
            usec,
            event_type,
            code,
            value,
        }
    }

    /// The event's timestamp as a [`TraceTime`].
    #[must_use]
    pub const fn time(&self) -> TraceTime {
        TraceTime {
            sec: self.sec,
            usec: self.usec,
        }
    }

    /// Structural field validation (ranges only, not monotonicity).
    ///
    /// Rejects out-of-range `usec` and timestamps that cannot convert to a
    /// core [`Monotonic`]. The writer and the reader both call this so an
    /// invalid value can never be written to or accepted from a trace.
    /// Monotonic *ordering* is deliberately not checked here: the writer
    /// records timestamps faithfully, while the reader enforces the
    /// non-decreasing policy separately ([`crate::TraceError::TimeRegression`]).
    pub fn validate_fields(&self) -> Result<(), TraceFieldError> {
        if (self.usec as u64) >= USEC_PER_SEC {
            return Err(TraceFieldError::UsecOutOfRange(self.usec));
        }
        if self.time().to_nanos().is_none() {
            return Err(TraceFieldError::TimeOverflow {
                sec: self.sec,
                usec: self.usec,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_accepts_kernel_style_event() {
        let event = TraceEvent::new(0, 1234, 3, 47, 0);
        assert_eq!(event.validate_fields(), Ok(()));
        assert_eq!(event.time(), TraceTime { sec: 0, usec: 1234 });
    }

    #[test]
    fn validation_rejects_bad_usec() {
        assert_eq!(
            TraceEvent::new(0, 1_000_000, 3, 47, 0).validate_fields(),
            Err(TraceFieldError::UsecOutOfRange(1_000_000))
        );
    }

    #[test]
    fn validation_rejects_unconvertible_time() {
        let event = TraceEvent::new(u64::MAX, 0, 3, 47, 0);
        assert!(matches!(
            event.validate_fields(),
            Err(TraceFieldError::TimeOverflow { .. })
        ));
    }

    #[test]
    fn json_round_trip_uses_flat_type_field() {
        let event = TraceEvent::new(1, 2, 3, 47, -1);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":3"));
        assert!(json.contains("\"code\":47"));
        assert!(json.contains("\"value\":-1"));
        assert_eq!(serde_json::from_str::<TraceEvent>(&json).unwrap(), event);
    }
}
