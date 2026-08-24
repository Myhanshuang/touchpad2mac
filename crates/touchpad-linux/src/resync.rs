//! Mockable kernel-state snapshot boundary for `SYN_DROPPED` recovery.
//!
//! When the input stream loses continuity, the decoder must rebuild its slot
//! state from the kernel instead of trusting the (possibly incomplete)
//! incremental events it is ignoring. That kernel query is abstracted behind
//! [`ResyncSource`], so tests and offline replay can inject a
//! [`KernelStateSnapshot`] without any `/dev/input` access. M4 will provide
//! the real ioctl-backed adapter; M3 only defines the boundary and the mocked
//! behavior.
//!
//! The decoder requires a **complete, internally consistent** snapshot: a
//! snapshot containing an out-of-range or duplicate slot, a tracking id
//! below `-1`, or an active contact missing its raw X or Y coordinate is
//! rejected as a resync failure (the decoder degrades and publishes no
//! frame).
#![forbid(unsafe_code)]

use std::error::Error as StdError;

use touchpad_core::{PhysicalButtons, RawAxis};

/// A source that can produce the kernel's current input state on demand.
///
/// Implementations must return a complete, internally consistent snapshot:
/// every slot that currently holds a contact, its tracking id, all reported
/// `ABS_MT_*` fields, and the physical button state. The decoder normalizes
/// the raw values with the device descriptor it was configured with.
///
/// The failure type is a boxed error (rather than an associated type) so the
/// trait stays object-safe and the decoder can store it as
/// `Box<dyn ResyncSource>`; concrete implementations (mocks, the M4 ioctl
/// adapter) return their own error types converted into the box.
pub trait ResyncSource {
    /// Queries the kernel's current state.
    fn snapshot(&mut self) -> Result<KernelStateSnapshot, Box<dyn StdError + Send + Sync>>;
}

/// Complete input state read from the kernel during resynchronization.
///
/// Both active and empty slots may be listed; the decoder only publishes
/// slots whose snapshot `tracking_id` is non-negative.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KernelStateSnapshot {
    /// Physical button state at the moment of the snapshot.
    pub physical_buttons: PhysicalButtons,
    /// The device's slots.
    pub slots: Vec<SlotSnapshot>,
}

impl KernelStateSnapshot {
    /// Creates a snapshot.
    #[must_use]
    pub fn new(physical_buttons: PhysicalButtons, slots: Vec<SlotSnapshot>) -> Self {
        Self {
            physical_buttons,
            slots,
        }
    }
}

/// One slot's state as read from the kernel.
///
/// A valid snapshot lists each slot at most once, with a tracking id of
/// either `>= 0` (active — raw X and Y are then required) or exactly `-1`
/// (empty). Anything else is rejected by the decoder as an invalid snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotSnapshot {
    /// Type-B slot index.
    pub slot: u32,
    /// Tracking id; `-1` marks an empty slot, `>= 0` an active contact.
    /// Values below `-1` are invalid and rejected by the decoder.
    pub tracking_id: i32,
    /// Raw `ABS_MT_POSITION_X` value, when the slot reports one.
    pub position_x: Option<RawAxis>,
    /// Raw `ABS_MT_POSITION_Y` value, when the slot reports one.
    pub position_y: Option<RawAxis>,
    /// Raw `ABS_MT_PRESSURE` value, when the slot reports one.
    pub pressure: Option<RawAxis>,
    /// Raw `ABS_MT_TOUCH_MAJOR` value, when the slot reports one.
    pub touch_major: Option<RawAxis>,
    /// Raw `ABS_MT_TOUCH_MINOR` value, when the slot reports one.
    pub touch_minor: Option<RawAxis>,
    /// Raw `ABS_MT_ORIENTATION` value, when the slot reports one.
    pub orientation: Option<RawAxis>,
}

impl SlotSnapshot {
    /// Creates an empty slot snapshot (no optional fields).
    #[must_use]
    pub fn new(slot: u32, tracking_id: i32) -> Self {
        Self {
            slot,
            tracking_id,
            position_x: None,
            position_y: None,
            pressure: None,
            touch_major: None,
            touch_minor: None,
            orientation: None,
        }
    }
}
