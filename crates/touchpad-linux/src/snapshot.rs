//! Real `SYN_DROPPED` snapshot adapter (M4 requirement 3).
//!
//! [`EvdevSnapshotSource`] implements the M3 [`ResyncSource`] boundary with
//! the real kernel protocol: when the decoder loses continuity, the source
//! reads the kernel's current per-slot state via `EVIOCGMTSLOTS` (one ioctl
//! per `ABS_MT_*` axis returns the value for every slot) and the physical
//! button state via `EVIOCGKEY`, all through the mockable [`Sys`] seam.
//!
//! Guarantees (matching M3's complete-snapshot contract):
//!
//! * **Bounded**: the slot count is validated against
//!   [`crate::MAX_SLOT_COUNT`] at construction, so the ioctl buffers and the
//!   returned snapshot can never be oversized.
//! * **Complete**: the snapshot lists every slot (active or empty) and every
//!   `ABS_MT_*` axis the device reported during probing.
//! * **Fail-closed**: any ioctl failure, any `EVIOCGKEY` response too short
//!   to cover the consumed physical buttons, or any invalid tracking id
//!   makes [`ResyncSource::snapshot`] return an error, which drives the
//!   decoder into `Degraded` with **no frame published** (M3 review R4; M4
//!   review R7). The full validity check (duplicate slots, missing raw X/Y
//!   for active contacts, ...) remains the decoder's `apply_snapshot` step,
//!   so the snapshot is validated again before any live state is touched.
//!
//! ## `EVIOCGMTSLOTS` buffer protocol (M4 review R2/RR2)
//!
//! The ioctl takes a buffer whose first element is the `ABS_MT_*` code to
//! query; the kernel reads it, writes one value per slot after it, and
//! returns **0 on success** (`drivers/input/evdev.c::evdev_handle_mt_request`
//! — not a byte count). The required buffer is therefore one leading code
//! plus `slot_count` values: `(slot_count + 1)` `i32`s. The kernel computes
//! `max_slots = (size - sizeof(__u32)) / sizeof(__s32)` and writes
//! `min(num_slots, max_slots)` values — a buffer that holds fewer slots
//! than the device **truncates the response rather than erroring** (M4
//! review RR2; there is no `-EINVAL` size-mismatch rejection).
//!
//! Completeness for the production adapter rests on the **kernel invariant
//! that on a given fd `num_slots == ABS_MT_SLOT.max + 1`**
//! (`input_mt_init_slots` derives the axis maximum from the slot count).
//! The adapter's `slot_count` is read from `ABS_MT_SLOT`'s
//! `absinfo.max + 1` on the **same open fd** (M4 review R4) and bounded by
//! [`crate::MAX_SLOT_COUNT`], so it matches the kernel's `num_slots` and a
//! successful `(slot_count + 1)`-element call fully populates
//! `buf[1..=slot_count]`. The ioctl's return value is neither needed nor
//! usable to validate the slot count.
//!
//! ## `EVIOCGKEY` completeness (M4 review R7/RR1)
//!
//! The resync consumes `BTN_LEFT` through `BTN_MIDDLE`. `EVIOCGKEY` returns
//! the **number of bytes copied** as its ioctl result (kernel
//! `evdev_handle_get_val` → `bits_to_user`; it does not return 0 on
//! success), which is the whole key array when the buffer is large enough.
//! A response shorter than the bytes covering `BTN_MIDDLE` would
//! silently read as "nothing pressed" out of a zero-filled buffer, which is
//! unsafe while a physical button is held. Such a response fails the
//! snapshot closed.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::rc::Rc;

use touchpad_core::{PhysicalButtons, RawAxis};

use crate::codes::{
    bits_to_bytes, test_bit, ABS_MT_ORIENTATION, ABS_MT_POSITION_X, ABS_MT_POSITION_Y,
    ABS_MT_PRESSURE, ABS_MT_TOUCH_MAJOR, ABS_MT_TOUCH_MINOR, ABS_MT_TRACKING_ID, BTN_LEFT,
    BTN_MIDDLE, BTN_RIGHT, KEY_MAX,
};
use crate::resync::{KernelStateSnapshot, ResyncSource, SlotSnapshot};
use crate::sys::{Fd, Sys, SysError};
use crate::MAX_SLOT_COUNT;

/// Failure of a real kernel snapshot read. Every variant is fatal for the
/// resync (the decoder degrades and publishes no frame).
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// `EVIOCGMTSLOTS` failed for an axis.
    #[error("EVIOCGMTSLOTS for axis {code} failed: {source}")]
    MtSlots {
        /// The ABS code that was queried.
        code: u16,
        /// Why the ioctl failed.
        source: SysError,
    },
    /// `EVIOCGKEY` failed.
    #[error("EVIOCGKEY failed: {0}")]
    KeyState(SysError),
    /// `EVIOCGKEY` returned fewer bytes than needed to cover the physical
    /// buttons the snapshot consumes (`BTN_LEFT` through `BTN_MIDDLE`). A
    /// truncated response must fail closed rather than silently read as
    /// "nothing pressed" (M4 review R7).
    #[error(
        "EVIOCGKEY returned {returned} bytes; the snapshot needs at least {required} bytes to cover BTN_LEFT..BTN_MIDDLE"
    )]
    KeyStateTruncated {
        /// Bytes the device reported.
        returned: usize,
        /// Minimum bytes covering `BTN_MIDDLE` inclusive.
        required: usize,
    },
    /// The kernel reported a tracking id below `-1`.
    #[error("snapshot reports invalid tracking id {tracking_id} for slot {slot}")]
    InvalidTrackingId {
        /// The slot index.
        slot: u32,
        /// The invalid tracking id.
        tracking_id: i32,
    },
    /// The slot count is outside the decoder's supported range.
    #[error("slot count {slot_count} is outside the supported range [1, {max}]")]
    SlotCountOutOfRange {
        /// The requested slot count.
        slot_count: u32,
        /// The supported maximum.
        max: u32,
    },
}

/// Real kernel-state snapshot source for `SYN_DROPPED` recovery.
pub struct EvdevSnapshotSource {
    sys: Rc<dyn Sys>,
    fd: Fd,
    slot_count: u32,
    /// `ABS_MT_*` axes (besides `ABS_MT_TRACKING_ID`) to read per slot.
    mt_axes: Vec<u16>,
    /// Whether the device reports physical buttons (skips `EVIOCGKEY` when
    /// false).
    has_buttons: bool,
}

impl EvdevSnapshotSource {
    /// Creates the adapter for an open device.
    ///
    /// `slot_count` must lie in `[1, MAX_SLOT_COUNT]`; `mt_axes` lists the
    /// `ABS_MT_*` codes the device reports (deduplicated internally).
    pub fn new(
        sys: Rc<dyn Sys>,
        fd: Fd,
        slot_count: u32,
        mt_axes: impl IntoIterator<Item = u16>,
        has_buttons: bool,
    ) -> Result<Self, SnapshotError> {
        if slot_count == 0 || slot_count > MAX_SLOT_COUNT {
            return Err(SnapshotError::SlotCountOutOfRange {
                slot_count,
                max: MAX_SLOT_COUNT,
            });
        }
        let mut axes: Vec<u16> = mt_axes.into_iter().collect();
        axes.sort_unstable();
        axes.dedup();
        Ok(Self {
            sys,
            fd,
            slot_count,
            mt_axes: axes,
            has_buttons,
        })
    }

    /// Reads the per-slot values of one `ABS_MT_*` axis via
    /// `EVIOCGMTSLOTS`.
    ///
    /// The buffer is the ABI-correct `slot_count + 1` `i32`s (leading code +
    /// one value per slot). The kernel returns 0 on success and writes
    /// `min(num_slots, max_slots)` values; because the same-fd kernel
    /// invariant gives `num_slots == ABS_MT_SLOT.max + 1 == slot_count`,
    /// the full `buf[1..=slot_count]` is populated and no byte-count
    /// validation is possible or needed (M4 reviews R2/RR2).
    fn read_mt_axis(&self, code: u16) -> Result<Vec<i32>, SnapshotError> {
        let mut buf = vec![0i32; self.slot_count as usize + 1];
        buf[0] = code as i32;
        self.sys
            .ioctl_mt_slots(self.fd, &mut buf)
            .map_err(|source| SnapshotError::MtSlots { code, source })?;
        Ok(buf[1..].to_vec())
    }

    /// Reads the physical button state via `EVIOCGKEY`, requiring the
    /// response to cover every consumed button bit (`BTN_LEFT` through
    /// `BTN_MIDDLE`); a short response fails the snapshot closed (M4 review
    /// R7).
    fn read_buttons(&self) -> Result<PhysicalButtons, SnapshotError> {
        let mut state = vec![0u8; bits_to_bytes(KEY_MAX)];
        let returned = self
            .sys
            .ioctl_key_state(self.fd, &mut state)
            .map_err(SnapshotError::KeyState)?;
        let required = BTN_MIDDLE as usize / 8 + 1;
        if returned < required {
            return Err(SnapshotError::KeyStateTruncated { returned, required });
        }
        Ok(PhysicalButtons::new(
            test_bit(&state, BTN_LEFT),
            test_bit(&state, BTN_RIGHT),
            test_bit(&state, BTN_MIDDLE),
        ))
    }
}

impl ResyncSource for EvdevSnapshotSource {
    fn snapshot(&mut self) -> Result<KernelStateSnapshot, Box<dyn StdError + Send + Sync>> {
        let slot_count = self.slot_count as usize;

        let tracking = self.read_mt_axis(ABS_MT_TRACKING_ID)?;
        let mut values: BTreeMap<u16, Vec<i32>> = BTreeMap::new();
        for code in &self.mt_axes {
            values.insert(*code, self.read_mt_axis(*code)?);
        }
        let buttons = if self.has_buttons {
            self.read_buttons()?
        } else {
            PhysicalButtons::NONE
        };

        let mut slots = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            let tracking_id = tracking[index];
            if tracking_id < -1 {
                return Err(SnapshotError::InvalidTrackingId {
                    slot: index as u32,
                    tracking_id,
                }
                .into());
            }
            let mut slot = SlotSnapshot::new(index as u32, tracking_id);
            if tracking_id >= 0 {
                slot.position_x = values
                    .get(&ABS_MT_POSITION_X)
                    .map(|values| RawAxis::new(values[index]));
                slot.position_y = values
                    .get(&ABS_MT_POSITION_Y)
                    .map(|values| RawAxis::new(values[index]));
                slot.pressure = values
                    .get(&ABS_MT_PRESSURE)
                    .map(|values| RawAxis::new(values[index]));
                slot.touch_major = values
                    .get(&ABS_MT_TOUCH_MAJOR)
                    .map(|values| RawAxis::new(values[index]));
                slot.touch_minor = values
                    .get(&ABS_MT_TOUCH_MINOR)
                    .map(|values| RawAxis::new(values[index]));
                slot.orientation = values
                    .get(&ABS_MT_ORIENTATION)
                    .map(|values| RawAxis::new(values[index]));
            }
            slots.push(slot);
        }

        Ok(KernelStateSnapshot::new(buttons, slots))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::sys::mock::{MockDevice, MockFailure, MockSys};

    /// A mock device with two active contacts (slots 0 and 2) and one empty
    /// slot, plus per-axis values.
    fn populated_device() -> MockDevice {
        let mut device = MockDevice::touchpad("pad", 4);
        device.set_mt_slots(ABS_MT_TRACKING_ID, vec![10, -1, 20, -1]);
        device.set_mt_slots(ABS_MT_POSITION_X, vec![100, 0, 300, 0]);
        device.set_mt_slots(ABS_MT_POSITION_Y, vec![50, 0, 150, 0]);
        device.set_key_state(BTN_LEFT, true);
        device.set_key_state(BTN_RIGHT, false);
        device
    }

    fn source(sys: &Rc<MockSys>, fd: Fd) -> EvdevSnapshotSource {
        let sys: Rc<dyn Sys> = sys.clone();
        EvdevSnapshotSource::new(sys, fd, 4, [ABS_MT_POSITION_X, ABS_MT_POSITION_Y], true).unwrap()
    }

    /// Expects `EvdevSnapshotSource::new` to fail and returns the error.
    fn new_source_error(
        sys: &Rc<MockSys>,
        fd: Fd,
        slot_count: u32,
        axes: [u16; 0],
        has_buttons: bool,
    ) -> SnapshotError {
        let sys: Rc<dyn Sys> = sys.clone();
        match EvdevSnapshotSource::new(sys, fd, slot_count, axes, has_buttons) {
            Err(error) => error,
            Ok(_) => panic!("expected construction to fail"),
        }
    }

    #[test]
    fn snapshot_reads_all_slots_axes_and_buttons() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, populated_device());
        let fd = sys.open(&path).unwrap();
        let mut source = source(&sys, fd);
        let snapshot = source.snapshot().unwrap();

        assert_eq!(
            snapshot.physical_buttons,
            PhysicalButtons::new(true, false, false)
        );
        assert_eq!(snapshot.slots.len(), 4);
        // Slot 0: active with coordinates.
        assert_eq!(snapshot.slots[0].tracking_id, 10);
        assert_eq!(snapshot.slots[0].position_x, Some(RawAxis::new(100)));
        assert_eq!(snapshot.slots[0].position_y, Some(RawAxis::new(50)));
        // Slot 1: empty.
        assert_eq!(snapshot.slots[1].tracking_id, -1);
        // Slot 2: active.
        assert_eq!(snapshot.slots[2].tracking_id, 20);
        assert_eq!(snapshot.slots[2].position_x, Some(RawAxis::new(300)));
        assert_eq!(snapshot.slots[2].position_y, Some(RawAxis::new(150)));
        // Slot 3: empty.
        assert_eq!(snapshot.slots[3].tracking_id, -1);
    }

    #[test]
    fn snapshot_without_buttons_skips_key_state() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, populated_device());
        let fd = sys.open(&path).unwrap();
        let sys_rc: Rc<dyn Sys> = sys.clone();
        let mut source =
            EvdevSnapshotSource::new(sys_rc, fd, 4, [ABS_MT_POSITION_X, ABS_MT_POSITION_Y], false)
                .unwrap();
        let snapshot = source.snapshot().unwrap();
        assert_eq!(snapshot.physical_buttons, PhysicalButtons::NONE);
        // EVIOCGKEY must not have been called.
        assert_eq!(
            sys.count(|call| matches!(call, crate::sys::mock::MockCall::KeyState(_))),
            0
        );
    }

    #[test]
    fn mt_slots_ioctl_failure_fails_the_snapshot() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = populated_device();
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let fd = sys.open(&path).unwrap();
        let mut source = source(&sys, fd);
        let err = source.snapshot().unwrap_err();
        assert!(err.to_string().contains("EVIOCGMTSLOTS"), "{err}");
    }

    /// M4 review R2 regression: the kernel's `evdev_handle_mt_request`
    /// returns 0 on success (never a byte count), and the adapter must
    /// accept that — the old seam required `slot_count * 4` returned bytes
    /// and would have failed every real resync.
    #[test]
    fn mt_slots_success_at_the_ffi_boundary_is_zero() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, populated_device());
        let fd = sys.open(&path).unwrap();
        // Direct seam check: success is `Ok(())`, and the values land in
        // `buf[1..]` of a `slot_count + 1` element buffer.
        let mut buf = [0i32; 5];
        buf[0] = ABS_MT_TRACKING_ID as i32;
        sys.ioctl_mt_slots(fd, &mut buf).unwrap();
        assert_eq!(&buf[1..], &[10, -1, 20, -1]);
        // And the adapter's snapshot succeeds end to end with that same
        // zero-return protocol.
        let mut source = source(&sys, fd);
        assert!(source.snapshot().is_ok());
    }

    /// M4 review RR2 regression: the kernel's `evdev_handle_mt_request`
    /// writes `min(num_slots, max_slots)` values and returns 0 — it does
    /// **not** reject an oversized device with `-EINVAL`. The mock mirrors
    /// that truncation, and production completeness rests on the same-fd
    /// kernel invariant (`num_slots == ABS_MT_SLOT.max + 1 == slot_count`),
    /// never on an invented error.
    #[test]
    fn mt_slots_truncates_values_that_do_not_fit_like_the_kernel() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = populated_device();
        // The descriptor says 4 slots, but the device's stored values hold
        // 5: `evdev_handle_mt_request` copies the first `max_slots = 4`
        // values and still returns 0.
        device.set_mt_slots(ABS_MT_TRACKING_ID, vec![10, -1, 20, -1, 30]);
        sys.add_device(&path, device);
        let fd = sys.open(&path).unwrap();

        // Seam level: the mock truncates to the buffer's capacity and
        // succeeds — no invented `-EINVAL`.
        let mut buf = [0i32; 5];
        buf[0] = ABS_MT_TRACKING_ID as i32;
        sys.ioctl_mt_slots(fd, &mut buf).unwrap();
        assert_eq!(&buf[1..], &[10, -1, 20, -1]);

        // Adapter level: the snapshot still succeeds; the values that fit
        // are read and completeness is guaranteed by the same-fd invariant,
        // not by the ioctl's return value.
        let mut source = source(&sys, fd);
        let snapshot = source.snapshot().unwrap();
        assert_eq!(snapshot.slots[0].tracking_id, 10);
        assert_eq!(snapshot.slots[2].tracking_id, 20);
    }

    #[test]
    fn invalid_tracking_id_fails_the_snapshot() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = populated_device();
        device.set_mt_slots(ABS_MT_TRACKING_ID, vec![10, -2, -1, -1]);
        sys.add_device(&path, device);
        let fd = sys.open(&path).unwrap();
        let mut source = source(&sys, fd);
        let err = source.snapshot().unwrap_err();
        assert!(err.to_string().contains("invalid tracking id -2"), "{err}");
    }

    /// M4 review R7: a truncated `EVIOCGKEY` response (shorter than the
    /// bytes covering `BTN_LEFT..BTN_MIDDLE`) must fail the snapshot closed
    /// instead of silently reading as "nothing pressed".
    #[test]
    fn short_key_state_response_fails_the_snapshot() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = populated_device();
        // 34 bytes covers BTN_LEFT/BTN_RIGHT but not BTN_MIDDLE (byte 34).
        device.key_state = vec![0u8; 34];
        sys.add_device(&path, device);
        let fd = sys.open(&path).unwrap();
        let mut source = source(&sys, fd);
        let err = source.snapshot().unwrap_err();
        assert!(
            err.to_string().contains("EVIOCGKEY returned 34 bytes"),
            "{err}"
        );
        // A full 96-byte response at the boundary is accepted.
        let sys2 = Rc::new(MockSys::new());
        let path2 = PathBuf::from("/dev/input/event0");
        let mut device2 = populated_device();
        device2.key_state = vec![0u8; 96];
        sys2.add_device(&path2, device2);
        let fd2 = sys2.open(&path2).unwrap();
        let sys_rc: Rc<dyn Sys> = sys2.clone();
        let mut source2 =
            EvdevSnapshotSource::new(sys_rc, fd2, 4, [ABS_MT_POSITION_X, ABS_MT_POSITION_Y], true)
                .unwrap();
        assert!(source2.snapshot().is_ok());
    }

    #[test]
    fn construction_rejects_out_of_range_slot_counts() {
        let sys = Rc::new(MockSys::new());
        let err = new_source_error(&sys, Fd::new(0), 0, [], false);
        assert!(
            err.to_string().contains("outside the supported range"),
            "{err}"
        );

        let err = new_source_error(&sys, Fd::new(0), MAX_SLOT_COUNT + 1, [], false);
        assert!(
            err.to_string().contains("outside the supported range"),
            "{err}"
        );

        // The boundary value is accepted.
        let sys_rc: Rc<dyn Sys> = sys.clone();
        EvdevSnapshotSource::new(sys_rc, Fd::new(0), MAX_SLOT_COUNT, [], false).unwrap();
    }

    #[test]
    fn snapshot_is_queryable_multiple_times() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, populated_device());
        let fd = sys.open(&path).unwrap();
        let mut source = source(&sys, fd);
        let first = source.snapshot().unwrap();
        let second = source.snapshot().unwrap();
        assert_eq!(first, second);
    }
}
