//! Monotonic time for gesture timing.
//!
//! Wall-clock time must never participate in timeout or velocity math; only
//! [`Monotonic`] values do.
//!
//! Clock-domain policy: `touchpad-core` never reads a clock itself. The
//! platform input layer stamps frames with kernel/`CLOCK_MONOTONIC`
//! timestamps, and the offline replay boundary will supply the same values
//! from a trace. [`Monotonic`] only represents and checks arithmetic on
//! those externally supplied timestamps, so there is exactly one time domain
//! in the core and no `now()`-style process-local clock to mix into it. A
//! single interaction must use timestamps from one domain only (live or
//! replay), never a blend; future milestones that introduce a second domain
//! (e.g. a trace clock) must model it as a distinct, non-mixable type.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A nanosecond-precision timestamp from a monotonic clock, supplied by the
/// platform input layer or a trace.
///
/// The core does not read any clock: values are produced at the input
/// boundary (e.g. the kernel evdev timestamp) and passed in. This type only
/// represents the value and the checked arithmetic on it.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Monotonic(u64);

impl Monotonic {
    /// The zero timestamp (the origin of a monotonic timeline).
    pub const ZERO: Monotonic = Monotonic(0);

    /// Creates a timestamp from nanoseconds.
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// The timestamp in nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Checked elapsed time since `earlier`.
    ///
    /// Returns `None` when `earlier` is strictly after `self`, i.e. the
    /// monotonic clock went backwards — callers must surface that as a
    /// diagnostic instead of producing a negative duration.
    #[must_use]
    pub fn duration_since(self, earlier: Monotonic) -> Option<Duration> {
        self.0.checked_sub(earlier.0).map(Duration::from_nanos)
    }

    /// Checked addition; `None` on overflow.
    #[must_use]
    pub fn checked_add(self, duration: Duration) -> Option<Monotonic> {
        let nanos = u64::try_from(duration.as_nanos()).ok()?;
        self.0.checked_add(nanos).map(Self)
    }

    /// Saturating addition.
    #[must_use]
    pub fn saturating_add(self, duration: Duration) -> Monotonic {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(nanos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nanos_round_trip() {
        let t = Monotonic::from_nanos(1_000_000);
        assert_eq!(t.as_nanos(), 1_000_000);
    }

    #[test]
    fn duration_since_works() {
        let a = Monotonic::from_nanos(100);
        let b = Monotonic::from_nanos(250);
        assert_eq!(b.duration_since(a), Some(Duration::from_nanos(150)));
        // Time regression -> None, never a negative duration.
        assert_eq!(a.duration_since(b), None);
    }

    #[test]
    fn checked_add_overflow() {
        let t = Monotonic::from_nanos(u64::MAX);
        assert_eq!(t.checked_add(Duration::from_nanos(1)), None);
        assert_eq!(
            t.saturating_add(Duration::from_nanos(1)).as_nanos(),
            u64::MAX
        );
    }

    #[test]
    fn timestamps_are_external_values_not_a_local_clock() {
        // `Monotonic` has no `now()`: core code cannot read a clock, so a
        // process-local timebase can never be mixed with boundary-provided
        // kernel timestamps. Construction is only from explicit values.
        assert_eq!(Monotonic::from_nanos(0), Monotonic::ZERO);
        assert_eq!(
            Monotonic::from_nanos(123).checked_add(Duration::from_nanos(1)),
            Some(Monotonic::from_nanos(124))
        );
    }
}
