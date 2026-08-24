//! Trace timestamps: `(sec, usec)` pairs in a declared clock domain.
//!
//! Raw traces preserve the kernel-style `timeval` precision of the input
//! device (whole seconds + microseconds within the second). This module
//! defines the field ranges and the **checked** conversion to the core
//! [`Monotonic`] type.

use serde::{Deserialize, Serialize};

use touchpad_core::Monotonic;

/// Whole seconds in one second.
pub const USEC_PER_SEC: u64 = 1_000_000;

/// Nanoseconds in one second.
pub const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Microseconds in one millisecond.
const NANOS_PER_USEC: u64 = 1_000;

/// A trace timestamp: `sec` whole seconds plus `usec` microseconds.
///
/// ## Field ranges
///
/// * `sec`: non-negative whole seconds (`[0, u64::MAX]`).
/// * `usec`: microseconds within the second, `[0, 999_999]`. A value of
///   `1_000_000` or more is invalid: the reader/writer report it as an
///   invalid field instead of silently carrying into the next second.
///
/// The pair converts to a [`Monotonic`] timestamp of `sec * 1_000_000_000 +
/// usec * 1_000` nanoseconds. The conversion is **checked**:
/// [`TraceTime::to_monotonic`] returns `None` when the multiplication would
/// overflow `u64` (a `sec` beyond roughly `1.8e10` seconds, ~584 years of
/// uptime) or when `usec` is out of range. Callers must surface that as a
/// structured error, never as a wrapped/truncated value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TraceTime {
    /// Whole seconds; non-negative.
    pub sec: u64,
    /// Microseconds within the second; `[0, 999_999]`.
    pub usec: u32,
}

impl TraceTime {
    /// Whether `usec` is within `[0, 999_999]`.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        (self.usec as u64) < USEC_PER_SEC
    }

    /// Checked conversion to a core [`Monotonic`] timestamp.
    ///
    /// Returns `None` when `usec` is out of range or when
    /// `sec * 1_000_000_000 + usec * 1_000` overflows `u64`. This is the
    /// only path from a trace timestamp into core monotonic time; a `None`
    /// must become a structured error (the reader reports it as an invalid
    /// field), never a silently truncated timestamp.
    #[must_use]
    pub fn to_monotonic(&self) -> Option<Monotonic> {
        self.to_nanos().map(Monotonic::from_nanos)
    }

    /// Checked conversion to nanoseconds; `None` on out-of-range `usec` or
    /// `u64` overflow.
    #[must_use]
    pub fn to_nanos(&self) -> Option<u64> {
        if !self.is_valid() {
            return None;
        }
        let sec_nanos = self.sec.checked_mul(NANOS_PER_SEC)?;
        let usec_nanos = u64::from(self.usec).checked_mul(NANOS_PER_USEC)?;
        sec_nanos.checked_add(usec_nanos)
    }

    /// Converts a core [`Monotonic`] timestamp (nanosecond precision) into
    /// the trace's microsecond-precision representation.
    ///
    /// The trace format cannot represent sub-microsecond residues; they are
    /// truncated toward zero. A `Monotonic` that is microsecond-aligned
    /// round-trips exactly (`from_monotonic(t).to_monotonic() == Some(t)`).
    #[must_use]
    pub fn from_monotonic(timestamp: Monotonic) -> Self {
        let nanos = timestamp.as_nanos();
        Self {
            sec: nanos / NANOS_PER_SEC,
            usec: ((nanos % NANOS_PER_SEC) / NANOS_PER_USEC) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_range_checks() {
        assert!(TraceTime { sec: 0, usec: 0 }.is_valid());
        assert!(TraceTime {
            sec: 0,
            usec: 999_999
        }
        .is_valid());
        assert!(!TraceTime {
            sec: 0,
            usec: 1_000_000
        }
        .is_valid());
        assert!(!TraceTime {
            sec: 0,
            usec: u32::MAX
        }
        .is_valid());
    }

    #[test]
    fn conversion_is_exact_within_range() {
        let t = TraceTime {
            sec: 1,
            usec: 500_000,
        };
        assert_eq!(t.to_nanos(), Some(1_500_000_000));
        assert_eq!(t.to_monotonic(), Some(Monotonic::from_nanos(1_500_000_000)));
        assert_eq!(TraceTime { sec: 0, usec: 1 }.to_nanos(), Some(1_000));
    }

    #[test]
    fn out_of_range_usec_cannot_convert() {
        // An invalid usec must not silently wrap into the next second.
        assert_eq!(
            TraceTime {
                sec: 0,
                usec: 1_000_000
            }
            .to_monotonic(),
            None
        );
        assert_eq!(
            TraceTime {
                sec: 0,
                usec: 1_000_000
            }
            .to_nanos(),
            None
        );
    }

    #[test]
    fn overflow_cannot_convert() {
        let max = TraceTime {
            sec: u64::MAX,
            usec: 0,
        };
        assert_eq!(max.to_nanos(), None);
        assert_eq!(max.to_monotonic(), None);
        // The largest representable whole second converts...
        let boundary = TraceTime {
            sec: u64::MAX / NANOS_PER_SEC,
            usec: 0,
        };
        assert_eq!(
            boundary.to_nanos(),
            Some((u64::MAX / NANOS_PER_SEC) * NANOS_PER_SEC)
        );
        // ...but adding one second overflows...
        let one_more = TraceTime {
            sec: u64::MAX / NANOS_PER_SEC + 1,
            usec: 0,
        };
        assert_eq!(one_more.to_nanos(), None);
        // ...and so does a large usec on the largest whole second.
        let large_usec = TraceTime {
            sec: u64::MAX / NANOS_PER_SEC,
            usec: 999_999,
        };
        assert_eq!(large_usec.to_nanos(), None);
    }

    #[test]
    fn monotonic_round_trip_for_microsecond_aligned_values() {
        let t = Monotonic::from_nanos(123_456_789_000);
        let tt = TraceTime::from_monotonic(t);
        assert_eq!(
            tt,
            TraceTime {
                sec: 123,
                usec: 456_789
            }
        );
        assert_eq!(tt.to_monotonic(), Some(t));
    }

    #[test]
    fn sub_microsecond_residue_is_truncated() {
        // 123 us + 999 ns: the trace format keeps microsecond precision, so
        // the residue is truncated toward zero.
        let t = Monotonic::from_nanos(123_999);
        assert_eq!(
            TraceTime::from_monotonic(t),
            TraceTime { sec: 0, usec: 123 }
        );
    }

    #[test]
    fn ordering_follows_sec_then_usec() {
        let a = TraceTime {
            sec: 1,
            usec: 999_999,
        };
        let b = TraceTime { sec: 2, usec: 0 };
        let c = TraceTime { sec: 2, usec: 1 };
        assert!(a < b);
        assert!(b < c);
        assert!(a < c);
    }
}
