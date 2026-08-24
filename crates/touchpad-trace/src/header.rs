//! The trace header: schema version, clock semantics, and device descriptor.
//!
//! The header is the mandatory first line of a trace and must appear exactly
//! once. It captures everything a replay needs to interpret the raw events:
//! which schema version to apply, which clock domain the timestamps live in,
//! and the full device description (identity, capabilities, axis ranges,
//! resolutions, slot count) recorded at capture time.

use serde::{Deserialize, Serialize};

use touchpad_core::{DeviceDescriptor, Diagnostic};

use crate::SUPPORTED_SCHEMA_VERSION;

/// Clock domain of a trace's event timestamps.
///
/// Schema version 1 defines exactly one clock: [`TraceClock::Monotonic`].
/// The reader rejects any other value as an invalid field, because a trace
/// must never carry wall-clock semantics (design.md §15, IMPLEMENTATION_BRIEF
/// §4: gesture timing uses only the monotonic clock). Future schema versions
/// may add clocks; doing so is a structural change that must bump
/// `schema_version`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TraceClock {
    /// A monotonic clock; `(sec, usec)` timestamps convert directly to the
    /// core [`touchpad_core::Monotonic`] and never to wall time.
    Monotonic,
}

/// The mandatory first line of a trace.
///
/// Serialized as:
///
/// ```json
/// {"kind":"header","schema_version":1,"clock":"monotonic","device":{"name":"...","vendor_id":0,"product_id":0,"axes":{...},"slot_count":10,"supports_type_b_mt":true,"has_physical_buttons":true,"profile":{...}}}
/// ```
///
/// The device description is a **nested** `device` object (deliberately not
/// flattened): it carries the [`DeviceDescriptor`] recorded at capture time
/// — device identity, capabilities, axis ranges, resolutions, and slot
/// count. Replay (M3 decoder) must use this descriptor exactly like a live
/// device's descriptor, never a second model of the hardware. Nesting also
/// keeps schema-level header fields distinct from device fields and avoids
/// serde's flatten buffering, which cannot round-trip integer-like map keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TraceHeader {
    /// Trace schema version; must equal [`SUPPORTED_SCHEMA_VERSION`] (1).
    pub schema_version: u32,
    /// Clock domain of every event timestamp in this trace.
    pub clock: TraceClock,
    /// Device identity, capabilities, axes, resolutions, and slot count.
    pub device: DeviceDescriptor,
}

impl TraceHeader {
    /// Creates a schema-version-1 monotonic header for a device.
    #[must_use]
    pub fn new(device: DeviceDescriptor) -> Self {
        Self {
            schema_version: SUPPORTED_SCHEMA_VERSION,
            clock: TraceClock::Monotonic,
            device,
        }
    }

    /// Structural validation of the embedded device descriptor (axis ranges
    /// etc.), returning diagnostics instead of panicking.
    ///
    /// The reader/writer do not fail on these — they are data-quality
    /// diagnostics for downstream consumers (the replay decoder) — but
    /// callers can check them explicitly.
    #[must_use]
    pub fn validate(&self) -> Vec<Diagnostic> {
        self.device.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_header_is_schema_one_monotonic() {
        let header = TraceHeader::new(DeviceDescriptor::new("dev", 0, 0));
        assert_eq!(header.schema_version, SUPPORTED_SCHEMA_VERSION);
        assert_eq!(header.clock, TraceClock::Monotonic);
        assert!(header.validate().is_empty());
    }

    #[test]
    fn clock_serde_round_trip() {
        let json = serde_json::to_string(&TraceClock::Monotonic).unwrap();
        assert_eq!(json, "\"monotonic\"");
        assert_eq!(
            serde_json::from_str::<TraceClock>(&json).unwrap(),
            TraceClock::Monotonic
        );
    }

    #[test]
    fn header_round_trips_through_json() {
        let header = TraceHeader::new(DeviceDescriptor::new("dev", 0x1234, 0x5678));
        let json = serde_json::to_string(&header).unwrap();
        let decoded: TraceHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, header);
    }
}
