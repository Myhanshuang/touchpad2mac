//! Encoding of evdev `ioctl` request numbers (M4).
//!
//! The kernel encodes ioctl requests with the generic `_IOC` layout
//! (`asm-generic/ioctl.h`):
//!
//! ```text
//! _IOC(dir, type, nr, size) = (dir << 30) | (size << 16) | (type << 8) | nr
//! ```
//!
//! with `type == 'E'` (0x45) for evdev, `dir == _IOC_READ` (2) for
//! kernel→user queries, `dir == _IOC_WRITE` (1) for user→kernel commands,
//! and `size` the payload size in bytes. The parameterized evdev macros
//! (`EVIOCGNAME(len)`, `EVIOCGBIT(ev, len)`, `EVIOCGMTSLOTS(len)`,
//! `EVIOCGABS(abs)`, ...) are C macros, so the request must be computed
//! here; the encoders below are pure integer arithmetic and are unit-tested
//! against the canonical kernel values (`EVIOCGRAB == 0x40044590`,
//! `EVIOCGID == 0x80084502`, `EVIOCGABS(ABS_X) == 0x80184540`).
//!
//! This module is `unsafe`-free and platform-independent.

#![forbid(unsafe_code)]

/// `_IOC_WRITE` — userspace writes, kernel reads.
const IOC_WRITE: u32 = 1;
/// `_IOC_READ` — kernel writes, userspace reads.
const IOC_READ: u32 = 2;
/// `'E'` — the evdev ioctl type.
const EVDEV_TYPE: u32 = 0x45;

/// Builds a request from the `_IOC` fields.
#[must_use]
const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | (EVDEV_TYPE << 8) | nr
}

/// `EVIOCGVERSION` — get driver version (`_IOR('E', 0x01, int)`).
#[must_use]
pub const fn eviocgversion() -> u32 {
    ioc(IOC_READ, 0x01, 4)
}

/// `EVIOCGID` — get device id (`_IOR('E', 0x02, struct input_id)`, 8 bytes).
#[must_use]
pub const fn eviocgid() -> u32 {
    ioc(IOC_READ, 0x02, 8)
}

/// `EVIOCGNAME(len)` — get device name (`_IOC(READ, 'E', 0x06, len)`).
#[must_use]
pub const fn eviocgname(len: usize) -> u32 {
    ioc(IOC_READ, 0x06, len)
}

/// `EVIOCGPROP(len)` — get device properties (`_IOC(READ, 'E', 0x09, len)`).
#[must_use]
pub const fn eviocgprop(len: usize) -> u32 {
    ioc(IOC_READ, 0x09, len)
}

/// `EVIOCGMTSLOTS(len)` — get MT slot values (`_IOC(READ, 'E', 0x0a, len)`).
#[must_use]
pub const fn eviocgmt_slots(len: usize) -> u32 {
    ioc(IOC_READ, 0x0a, len)
}

/// `EVIOCGKEY(len)` — get global key state (`_IOC(READ, 'E', 0x18, len)`).
#[must_use]
pub const fn eviocgkey(len: usize) -> u32 {
    ioc(IOC_READ, 0x18, len)
}

/// `EVIOCGBIT(ev, len)` — get event bits (`_IOC(READ, 'E', 0x20 + ev, len)`).
#[must_use]
pub const fn eviocgbit(ev_type: u16, len: usize) -> u32 {
    ioc(IOC_READ, 0x20 + ev_type as u32, len)
}

/// `EVIOCGABS(abs)` — get abs value/limits (`_IOR('E', 0x40 + abs,
/// struct input_absinfo)`, 24 bytes).
#[must_use]
pub const fn eviocgabs(abs_code: u16) -> u32 {
    ioc(IOC_READ, 0x40 + abs_code as u32, 24)
}

/// `EVIOCSCLOCKID` — set the clock used for event timestamps
/// (`_IOW('E', 0xa0, __u32)`).
///
/// The evdev client clock defaults to `INPUT_CLK_REAL` (`CLOCK_REALTIME`,
/// value 0); the kernel switches it only after this ioctl. The runtime
/// issues `EVIOCSCLOCKID(CLOCK_MONOTONIC)` on its session fd before grab and
/// before any read (M4 review R1).
#[must_use]
pub const fn eviocsc_lockid() -> u32 {
    ioc(IOC_WRITE, 0xa0, 4)
}

/// `EVIOCGRAB` — grab/release the device (`_IOW('E', 0x90, int)`).
#[must_use]
pub const fn eviocgrab() -> u32 {
    ioc(IOC_WRITE, 0x90, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical kernel values these encodings must match
    /// (`linux/input.h` plus `asm-generic/ioctl.h`).
    #[test]
    fn canonical_evdev_requests() {
        // _IOR('E', 0x01, int)
        assert_eq!(eviocgversion(), 0x8004_4501);
        // _IOR('E', 0x02, struct input_id)  [8 bytes]
        assert_eq!(eviocgid(), 0x8008_4502);
        // _IOW('E', 0x90, int)
        assert_eq!(eviocgrab(), 0x4004_4590);
        // _IOW('E', 0xa0, __u32)
        assert_eq!(eviocsc_lockid(), 0x4004_45a0);
        // _IOR('E', 0x40 + 0x00, struct input_absinfo)  [24 bytes]  (ABS_X)
        assert_eq!(eviocgabs(0x00), 0x8018_4540);
        // ABS_MT_SLOT == 0x2f
        assert_eq!(eviocgabs(0x2f), 0x8018_456f);
        // ABS_MT_TRACKING_ID == 0x39
        assert_eq!(eviocgabs(0x39), 0x8018_4579);
    }

    #[test]
    fn parameterized_requests_follow_the_macro_layout() {
        // EVIOCGBIT(EV_ABS == 3, 8 bytes) -> nr 0x23, size 8
        assert_eq!(eviocgbit(0x03, 8), 0x8008_4523);
        // EVIOCGNAME(256 bytes) -> nr 0x06, size 256
        assert_eq!(eviocgname(256), 0x8100_4506);
        // EVIOCGKEY(96 bytes) -> nr 0x18, size 96
        assert_eq!(eviocgkey(96), 0x8060_4518);
        // EVIOCGPROP(4 bytes) -> nr 0x09, size 4
        assert_eq!(eviocgprop(4), 0x8004_4509);
        // EVIOCGMTSLOTS(4 * 258 bytes) -> nr 0x0a, size 1032
        assert_eq!(eviocgmt_slots(1032), 0x8408_450a);
    }

    #[test]
    fn size_encoding_is_in_bytes_not_shifted() {
        // A one-byte size must land in the size field, not overflow into nr.
        assert_eq!(eviocgname(1), 0x8001_4506);
        // The `type` field ('E' == 0x45) always occupies bits 8..16.
        assert_eq!(eviocgname(1) & 0x0000_ff00, 0x4500);
    }
}
