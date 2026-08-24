//! Safe, checked conversion of kernel `struct input_event` bytes into the
//! decoder's [`RawEvent`] boundary (M4).
//!
//! A raw `read(2)` from an evdev node yields whole `struct input_event`
//! structs whose layout is **conditionally defined** by the kernel UAPI
//! (`include/uapi/linux/input.h`):
//!
//! ```text
//! struct input_event {
//! #if (__BITS_PER_LONG != 32) || !defined(__USE_TIME_BITS64)
//!     struct timeval time;   // two `long`s: 16 bytes on 64-bit, 8 on 32-bit
//! #else
//!     __kernel_ulong_t __sec;    // 4 bytes on 32-bit (time64 ABI)
//!     __kernel_ulong_t __usec;
//! #endif
//!     __u16 type;
//!     __u16 code;
//!     __s32 value;
//! }
//! ```
//!
//! That conditionality — plus arch-specific `struct timeval` variants (e.g.
//! sparc64's 32-bit `tv_usec` plus padding) and the time64 ABI's unsigned
//! fields — means "Linux" alone does not determine the byte layout, and a
//! `size_of` assertion cannot catch a wrong *field interpretation* when the
//! total size happens to match. This module therefore implements and
//! verifies **exactly one live layout**: the x86_64 Linux ABI, where
//! `struct timeval` is two 8-byte `long`s and `struct input_event` is 24
//! bytes. Live Linux decoding on any other architecture fails at compile
//! time instead of silently misdecoding (M4 review RR3). Non-Linux targets
//! — offline replay and every mock test — remain portable and compile
//! unchanged; they exercise the same x86_64-shaped encoder/decoder so
//! mock bytes and live bytes agree.
//!
//! [`INPUT_EVENT_SIZE`] is therefore 24, [`KernelEvent::from_bytes`] reads
//! the two 8-byte time fields, and a compile-time assertion ties
//! [`INPUT_EVENT_SIZE`] to `size_of::<libc::input_event>()` on Linux.
//!
//! [`decode_input_events`] decodes a read buffer without `unsafe` (native
//! endianness byte reads), and [`KernelEvent::to_raw_event`] performs the
//! **checked** `timeval` → [`Monotonic`] conversion.
//!
//! ## Clock domain (M4 requirement 2; corrected per M4 review R1)
//!
//! The evdev client clock is zero-initialized to `INPUT_CLK_REAL`
//! (`CLOCK_REALTIME`, value 0) — evdev is **not** monotonic by
//! construction. The kernel switches the client to `CLOCK_MONOTONIC` only
//! after `EVIOCSCLOCKID(CLOCK_MONOTONIC)` succeeds
//! (`drivers/input/evdev.c::evdev_set_clk_type`), and the runtime issues
//! that ioctl on its session fd before grab and before reading any events.
//! Given that explicit setup, the `timeval` fields live in the **kernel
//! monotonic time domain**: whole non-negative seconds plus microseconds
//! within the second, and never wall clock. The conversion therefore:
//!
//! * rejects a negative `tv_sec` (a monotonic clock is never negative),
//! * rejects `tv_usec` outside `[0, 999_999]` (a value of `1_000_000` is an
//!   invalid field, never silently carried into the next second),
//! * rejects nanosecond overflow of `sec * 1_000_000_000 + usec * 1_000`,
//! * treats the resulting value strictly as monotonic nanoseconds — it is
//!   never converted to wall-clock time.
//!
//! A malformed timestamp means the kernel/driver stream is untrustworthy;
//! the runtime treats a conversion error as fatal for the stream (fail-open:
//! it stops producing frames and releases the grab).

use touchpad_core::Monotonic;

use crate::rawevent::RawEvent;

/// Size of one kernel `struct input_event` on the supported live Linux
/// target.
///
/// Live Linux FFI support is restricted to **x86_64** (M4 review RR3): the
/// UAPI layout is conditionally defined and the other variants (32-bit
/// `timeval` fields, the time64 ABI's unsigned `__kernel_ulong_t` fields,
/// sparc64's 32-bit `tv_usec` plus padding, ...) are not implemented or
/// verified here. On x86_64 Linux, `struct input_event` is 24 bytes: two
/// 8-byte `long` time fields followed by `type`/`code`/`value`.
pub const INPUT_EVENT_SIZE: usize = 24;

/// Live Linux FFI support restriction (M4 review RR3).
///
/// [`INPUT_EVENT_SIZE`] and [`KernelEvent::from_bytes`] implement and
/// verify only the **x86_64** Linux `struct input_event` layout. Other
/// Linux ABIs have layout variants (16-byte 32-bit `timeval`, time64
/// `__kernel_ulong_t` fields, sparc64 `usec`+padding, non-asm-generic
/// ioctl encodings) that this code does not decode — claiming "all
/// 32/64-bit Linux" would be false, so unsupported Linux targets fail at
/// compile time instead of silently misinterpreting fields. Non-Linux
/// targets (offline replay, the portable mock tests) are unaffected and
/// compile unchanged.
#[cfg(all(target_os = "linux", not(target_arch = "x86_64")))]
compile_error!(
    "touchpad-linux live Linux FFI is implemented and verified only for \
     x86_64 Linux: other Linux ABIs (32-bit timeval/time64, sparc64 \
     usec+padding, non-asm-generic ioctl encodings) are not supported. \
     Compile on x86_64 Linux, or use the portable offline replay/mock path."
);

/// Compile-time Linux ABI assertion: the byte layout assumed by
/// [`INPUT_EVENT_SIZE`]/[`KernelEvent::from_bytes`] must equal libc's
/// target-correct `struct input_event` size (M4 review R3). libc is a
/// Linux-only dependency, so this is gated on the supported live Linux
/// target.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const _: () = assert!(core::mem::size_of::<libc::input_event>() == INPUT_EVENT_SIZE);

/// Microseconds per second.
pub const USEC_PER_SEC: u64 = 1_000_000;

/// A decoded kernel `input_event` (the kernel layout, byte-decoded safely).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelEvent {
    /// `timeval.tv_sec` — whole seconds of the kernel monotonic clock.
    pub sec: i64,
    /// `timeval.tv_usec` — microseconds within the second.
    pub usec: i64,
    /// `input_event.type` — `EV_SYN`, `EV_KEY`, `EV_ABS`, ...
    pub event_type: u16,
    /// `input_event.code` — `SYN_REPORT`, `ABS_MT_*`, `BTN_*`, ...
    pub code: u16,
    /// `input_event.value` — signed 32-bit event value.
    pub value: i32,
}

/// Failure modes of [`decode_input_events`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EventDecodeError {
    /// The buffer length is not a multiple of [`INPUT_EVENT_SIZE`]; the
    /// kernel never produces torn events, so this indicates corrupt data
    /// (or a mock injecting one).
    #[error(
        "read returned {actual} bytes, which is not a multiple of the {expected}-byte input_event size; torn event data"
    )]
    BadLength {
        /// Bytes actually read.
        actual: usize,
        /// The expected per-event size.
        expected: usize,
    },
}

/// Failure modes of the `timeval` → [`Monotonic`] conversion.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimevalError {
    /// `tv_sec` is negative. Kernel `CLOCK_MONOTONIC` is never negative.
    #[error("negative tv_sec {0}: kernel monotonic time is never negative")]
    NegativeSeconds(i64),
    /// `tv_usec` is negative.
    #[error("negative tv_usec {0}")]
    NegativeMicroseconds(i64),
    /// `tv_usec` is `>= 1_000_000`; the value must be rejected, not carried
    /// into the next second.
    #[error("tv_usec {0} is outside [0, 999999]")]
    MicrosecondsOutOfRange(i64),
    /// `sec * 1_000_000_000 + usec * 1_000` overflows `u64` nanoseconds.
    #[error("timeval (sec {sec}, usec {usec}) overflows u64 nanoseconds")]
    NanosecondOverflow {
        /// The whole seconds field.
        sec: i64,
        /// The microseconds field.
        usec: i64,
    },
}

impl KernelEvent {
    /// Decodes one [`INPUT_EVENT_SIZE`]-byte chunk in the supported live
    /// x86_64 Linux layout (native endianness; M4 review RR3).
    ///
    /// # Panics
    ///
    /// Panics if `bytes.len() != INPUT_EVENT_SIZE`; callers must slice from a
    /// buffer already validated by [`decode_input_events`].
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(
            bytes.len(),
            INPUT_EVENT_SIZE,
            "input_event chunk must be {INPUT_EVENT_SIZE} bytes"
        );
        // Supported live layout (x86_64 Linux, M4 review RR3):
        // `struct timeval` carries two 8-byte `long` fields.
        let sec = i64::from_ne_bytes(bytes[0..8].try_into().expect("8-byte slice"));
        let usec = i64::from_ne_bytes(bytes[8..16].try_into().expect("8-byte slice"));
        let tail = &bytes[16..];
        let event_type = u16::from_ne_bytes(tail[0..2].try_into().expect("2-byte slice"));
        let code = u16::from_ne_bytes(tail[2..4].try_into().expect("2-byte slice"));
        let value = i32::from_ne_bytes(tail[4..8].try_into().expect("4-byte slice"));
        Self {
            sec,
            usec,
            event_type,
            code,
            value,
        }
    }

    /// Converts the kernel monotonic `timeval` into a core [`Monotonic`]
    /// timestamp, applying the checked field rules of this module.
    ///
    /// This is the only path from live kernel `timeval` data into core
    /// monotonic time; it never interprets the pair as wall clock.
    pub fn to_raw_event(&self) -> Result<RawEvent, TimevalError> {
        Ok(RawEvent::new(
            self.to_monotonic()?,
            self.event_type,
            self.code,
            self.value,
        ))
    }

    /// Converts this kernel event into the raw trace representation
    /// ([`touchpad_trace::TraceEvent`]) the recorder writes **before** the
    /// decoder sees it (M5: the raw recorder sits in front of the decoder, so
    /// a decoder bug cannot lose the raw input needed to reproduce it).
    ///
    /// Applies the same checked timeval field rules as
    /// [`KernelEvent::to_raw_event`] (`sec >= 0`, `0 <= usec < 1_000_000`);
    /// a `(sec, usec)` pair the trace schema cannot represent (negative or
    /// out-of-range) fails with the matching [`TimevalError`], so a recorder
    /// never writes an unrepresentable timestamp. The trace writer performs
    /// the remaining field validation (including the u64-nanosecond
    /// convertibility check) when the line is written.
    pub fn to_trace_event(&self) -> Result<touchpad_trace::TraceEvent, TimevalError> {
        if self.sec < 0 {
            return Err(TimevalError::NegativeSeconds(self.sec));
        }
        if self.usec < 0 {
            return Err(TimevalError::NegativeMicroseconds(self.usec));
        }
        if u64::try_from(self.usec).expect("checked non-negative above") >= USEC_PER_SEC {
            return Err(TimevalError::MicrosecondsOutOfRange(self.usec));
        }
        Ok(touchpad_trace::TraceEvent::new(
            u64::try_from(self.sec).expect("checked non-negative above"),
            self.usec as u32,
            self.event_type,
            self.code,
            self.value,
        ))
    }

    /// The checked `timeval` → [`Monotonic`] conversion.
    ///
    /// Mirrors [`touchpad_trace::TraceTime::to_monotonic`]'s semantics for the kernel-side
    /// pair so live and replayed timelines use identical conversion rules.
    pub fn to_monotonic(&self) -> Result<Monotonic, TimevalError> {
        if self.sec < 0 {
            return Err(TimevalError::NegativeSeconds(self.sec));
        }
        if self.usec < 0 {
            return Err(TimevalError::NegativeMicroseconds(self.usec));
        }
        let usec = u64::try_from(self.usec).expect("checked non-negative above");
        if usec >= USEC_PER_SEC {
            return Err(TimevalError::MicrosecondsOutOfRange(self.usec));
        }
        let sec = u64::try_from(self.sec).expect("checked non-negative above");
        let sec_nanos = sec
            .checked_mul(1_000_000_000)
            .ok_or(TimevalError::NanosecondOverflow {
                sec: self.sec,
                usec: self.usec,
            })?;
        let usec_nanos = usec.checked_mul(1_000).expect("usec < 1_000_000");
        let nanos = sec_nanos
            .checked_add(usec_nanos)
            .ok_or(TimevalError::NanosecondOverflow {
                sec: self.sec,
                usec: self.usec,
            })?;
        Ok(Monotonic::from_nanos(nanos))
    }
}

/// Decodes a raw `read(2)` buffer into whole kernel events.
///
/// The length must be a multiple of [`INPUT_EVENT_SIZE`]; a torn buffer is a
/// structured error ([`EventDecodeError::BadLength`]), never a panic or a
/// silently dropped tail.
pub fn decode_input_events(buf: &[u8]) -> Result<Vec<KernelEvent>, EventDecodeError> {
    if !buf.len().is_multiple_of(INPUT_EVENT_SIZE) {
        return Err(EventDecodeError::BadLength {
            actual: buf.len(),
            expected: INPUT_EVENT_SIZE,
        });
    }
    let mut events = Vec::with_capacity(buf.len() / INPUT_EVENT_SIZE);
    for chunk in buf.chunks_exact(INPUT_EVENT_SIZE) {
        events.push(KernelEvent::from_bytes(chunk));
    }
    Ok(events)
}

/// Encodes one kernel `struct input_event` in the supported live layout
/// (the inverse of [`KernelEvent::from_bytes`]).
///
/// Used by mocks and tests to produce x86_64-accurate raw bytes (M4 review
/// R3/RR3: the live layout is restricted to x86_64, with two 8-byte
/// `timeval` fields; the encoder and decoder always agree because they
/// implement the same single layout).
#[must_use]
pub fn encode_input_event(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
    // Supported live layout (x86_64 Linux): two 8-byte time fields.
    let mut bytes = Vec::with_capacity(INPUT_EVENT_SIZE);
    bytes.extend_from_slice(&sec.to_ne_bytes());
    bytes.extend_from_slice(&usec.to_ne_bytes());
    bytes.extend_from_slice(&event_type.to_ne_bytes());
    bytes.extend_from_slice(&code.to_ne_bytes());
    bytes.extend_from_slice(&value.to_ne_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use touchpad_trace::TraceTime;

    fn kernel_event(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> KernelEvent {
        KernelEvent {
            sec,
            usec,
            event_type,
            code,
            value,
        }
    }

    fn encode(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
        encode_input_event(sec, usec, event_type, code, value)
    }

    #[test]
    fn input_event_size_is_the_supported_x86_64_live_layout() {
        // The live Linux layout this crate implements and verifies is the
        // x86_64 one (M4 review RR3): 24 bytes, two 8-byte time fields.
        assert_eq!(INPUT_EVENT_SIZE, 24);
    }

    /// On the supported live Linux target the assumed layout must equal
    /// libc's `struct input_event`; the compile-time `const _` assertion
    /// covers it, this test re-checks it at runtime so the relationship is
    /// visible in the test output.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn input_event_abi_matches_libc_at_runtime() {
        assert_eq!(core::mem::size_of::<libc::input_event>(), INPUT_EVENT_SIZE);
    }

    #[test]
    fn decode_round_trip_and_multi_event_buffer() {
        let buf = [encode(1, 2, 3, 53, 100), encode(1, 3, 0, 0, 0)].concat();
        let events = decode_input_events(&buf).unwrap();
        assert_eq!(
            events,
            vec![kernel_event(1, 2, 3, 53, 100), kernel_event(1, 3, 0, 0, 0),]
        );
    }

    #[test]
    fn torn_buffer_is_a_structured_error() {
        let buf = encode(1, 2, 3, 53, 100);
        assert_eq!(
            decode_input_events(&buf[..INPUT_EVENT_SIZE - 10]),
            Err(EventDecodeError::BadLength {
                actual: INPUT_EVENT_SIZE - 10,
                expected: INPUT_EVENT_SIZE,
            })
        );
        assert_eq!(
            decode_input_events(&[]),
            Ok(vec![]),
            "an empty read is EOF, not a torn event"
        );
    }

    #[test]
    fn encode_decode_round_trip_uses_the_supported_layout() {
        // The encoder and decoder must agree on the single supported live
        // layout (24 bytes on x86_64 Linux).
        let bytes = encode(7, 500_000, 3, 53, 100);
        assert_eq!(bytes.len(), INPUT_EVENT_SIZE);
        assert_eq!(
            decode_input_events(&bytes).unwrap(),
            vec![kernel_event(7, 500_000, 3, 53, 100)]
        );
    }

    #[test]
    fn valid_timeval_converts_to_monotonic_nanos() {
        let event = kernel_event(1, 500_000, 3, 53, 100);
        assert_eq!(
            event.to_monotonic().unwrap(),
            Monotonic::from_nanos(1_500_000_000)
        );
        assert_eq!(
            event.to_raw_event().unwrap(),
            RawEvent::new(Monotonic::from_nanos(1_500_000_000), 3, 53, 100)
        );
    }

    #[test]
    fn negative_seconds_are_rejected() {
        let event = kernel_event(-1, 0, 3, 53, 0);
        assert_eq!(event.to_monotonic(), Err(TimevalError::NegativeSeconds(-1)));
    }

    #[test]
    fn usec_range_is_checked() {
        assert_eq!(
            kernel_event(0, -1, 3, 53, 0).to_monotonic(),
            Err(TimevalError::NegativeMicroseconds(-1))
        );
        assert_eq!(
            kernel_event(0, 1_000_000, 3, 53, 0).to_monotonic(),
            Err(TimevalError::MicrosecondsOutOfRange(1_000_000))
        );
        // The boundary value is accepted.
        assert_eq!(
            kernel_event(0, 999_999, 3, 53, 0).to_monotonic(),
            Ok(Monotonic::from_nanos(999_999_000))
        );
    }

    #[test]
    fn nanosecond_overflow_is_rejected_not_wrapped() {
        let event = kernel_event(i64::MAX, 0, 3, 53, 0);
        assert!(matches!(
            event.to_monotonic(),
            Err(TimevalError::NanosecondOverflow { .. })
        ));
        // A large-but-representable value converts exactly.
        let boundary = kernel_event(18_446_744_073, 0, 3, 53, 0);
        assert_eq!(
            boundary.to_monotonic(),
            Ok(Monotonic::from_nanos(18_446_744_073_000_000_000))
        );
    }

    #[test]
    fn live_conversion_matches_trace_conversion_rules() {
        // The same (sec, usec) pair must convert identically on the live and
        // the trace side (TraceTime uses the same checked math).
        for (sec, usec) in [(0, 0), (0, 999_999), (123, 456_789)] {
            let kernel = kernel_event(sec, usec, 3, 53, 0);
            let trace = TraceTime {
                sec: sec as u64,
                usec: usec as u32,
            };
            assert_eq!(
                kernel.to_monotonic().ok(),
                trace.to_monotonic(),
                "live and trace conversion must agree for (sec {sec}, usec {usec})"
            );
        }
    }

    #[test]
    fn to_trace_event_preserves_the_raw_event() {
        let event = kernel_event(1, 500_000, 3, 53, 100);
        let trace = event.to_trace_event().unwrap();
        assert_eq!(
            trace,
            touchpad_trace::TraceEvent::new(1, 500_000, 3, 53, 100)
        );
        // Round trip: the trace event converts back into the same raw event.
        assert_eq!(
            RawEvent::from_trace_event(&trace).unwrap(),
            event.to_raw_event().unwrap()
        );
    }

    #[test]
    fn to_trace_event_rejects_unrepresentable_timevals() {
        assert_eq!(
            kernel_event(-1, 0, 3, 53, 0).to_trace_event(),
            Err(TimevalError::NegativeSeconds(-1))
        );
        assert_eq!(
            kernel_event(0, -5, 3, 53, 0).to_trace_event(),
            Err(TimevalError::NegativeMicroseconds(-5))
        );
        assert_eq!(
            kernel_event(0, 1_000_000, 3, 53, 0).to_trace_event(),
            Err(TimevalError::MicrosecondsOutOfRange(1_000_000))
        );
        // The boundary value is accepted.
        assert_eq!(
            kernel_event(0, 999_999, 3, 53, 0)
                .to_trace_event()
                .unwrap()
                .usec,
            999_999
        );
    }
}
