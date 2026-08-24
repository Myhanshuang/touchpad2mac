//! Linux input event codes used by the Type-B decoder.
//!
//! Values follow the kernel `linux/input-event-codes.h` ABI
//! (<https://www.kernel.org/doc/html/latest/input/event-codes.html>). The
//! decoder matches on these constants to interpret raw events; they are
//! stable kernel ABI and are duplicated here so `touchpad-linux` needs no
//! FFI, system headers, or `/dev/input` access.
#![forbid(unsafe_code)]

use touchpad_core::AxisId;

/// `EV_SYN` — synchronization events.
pub const EV_SYN: u16 = 0x00;
/// `EV_KEY` — key/button events.
pub const EV_KEY: u16 = 0x01;
/// `EV_ABS` — absolute axis events.
pub const EV_ABS: u16 = 0x03;

/// `SYN_REPORT` — end of a frame; the decoder commits at this code.
pub const SYN_REPORT: u16 = 0x00;
/// `SYN_DROPPED` — the kernel dropped events; recovery is required.
pub const SYN_DROPPED: u16 = 0x03;

/// `ABS_MT_SLOT` — selects the current Type-B slot.
pub const ABS_MT_SLOT: u16 = 0x2f;
/// `ABS_MT_TOUCH_MAJOR` — contact ellipse major axis (raw units).
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
/// `ABS_MT_TOUCH_MINOR` — contact ellipse minor axis (raw units).
pub const ABS_MT_TOUCH_MINOR: u16 = 0x31;
/// `ABS_MT_ORIENTATION` — contact orientation (raw units).
pub const ABS_MT_ORIENTATION: u16 = 0x34;
/// `ABS_MT_POSITION_X` — contact X position (raw units).
pub const ABS_MT_POSITION_X: u16 = 0x35;
/// `ABS_MT_POSITION_Y` — contact Y position (raw units).
pub const ABS_MT_POSITION_Y: u16 = 0x36;
/// `ABS_MT_TRACKING_ID` — Type-B tracking id; `-1` ends the slot's contact.
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
/// `ABS_MT_PRESSURE` — contact pressure (raw units).
pub const ABS_MT_PRESSURE: u16 = 0x3a;

/// `BTN_LEFT` — left physical button.
pub const BTN_LEFT: u16 = 0x110;
/// `BTN_RIGHT` — right physical button.
pub const BTN_RIGHT: u16 = 0x111;
/// `BTN_MIDDLE` — middle physical button.
pub const BTN_MIDDLE: u16 = 0x112;

/// Highest `EV_*` code (`EV_MAX`); `BITS_TO_BYTES(EV_MAX + 1) == 4` bytes
/// cover the whole `evbit` array.
pub const EV_MAX: u16 = 0x1f;
/// Highest `KEY_*`/`BTN_*` code (`KEY_MAX`); `BITS_TO_BYTES(KEY_MAX + 1) ==
/// 96` bytes cover the whole `keybit` array.
pub const KEY_MAX: u16 = 0x2ff;
/// Highest `ABS_*` code (`ABS_MAX`); `BITS_TO_BYTES(ABS_MAX + 1) == 8` bytes
/// cover the whole `absbit` array.
pub const ABS_MAX: u16 = 0x3f;

/// `INPUT_PROP_POINTER` — the device needs a pointer (indirect pointing
/// device, i.e. a touchpad rather than a touchscreen).
pub const INPUT_PROP_POINTER: u16 = 0x00;
/// `INPUT_PROP_DIRECT` — direct input device (touchscreen, tablet).
pub const INPUT_PROP_DIRECT: u16 = 0x01;
/// `INPUT_PROP_BUTTONPAD` — the pad itself is the button (unified
/// buttonpad).
pub const INPUT_PROP_BUTTONPAD: u16 = 0x02;
/// `INPUT_PROP_SEMI_MT` — the device only reports a touch rectangle.
pub const INPUT_PROP_SEMI_MT: u16 = 0x03;
/// `INPUT_PROP_POINTING_STICK` — the device is a pointing stick.
pub const INPUT_PROP_POINTING_STICK: u16 = 0x05;
/// Highest `INPUT_PROP_*` code (`INPUT_PROP_MAX`);
/// `BITS_TO_BYTES(INPUT_PROP_MAX + 1) == 4` bytes cover the whole `propbit`
/// array.
pub const INPUT_PROP_MAX: u16 = 0x1f;

/// Number of bytes needed to hold bits `0..=max_bit` (kernel
/// `BITS_TO_BYTES(max_bit + 1)`).
#[must_use]
pub const fn bits_to_bytes(max_bit: u16) -> usize {
    (max_bit as usize / 8) + 1
}

/// Whether bit `bit` is set in a little-endian bit array (kernel
/// `test_bit`). Returns `false` when `bit` is outside `buf`.
#[must_use]
pub fn test_bit(buf: &[u8], bit: u16) -> bool {
    let byte = bit as usize / 8;
    let mask = 1u8 << (bit % 8);
    buf.get(byte).is_some_and(|value| value & mask != 0)
}

/// Sets bit `bit` in a little-endian bit array (kernel `set_bit`). No-op when
/// `bit` is outside `buf`.
pub fn set_bit(buf: &mut [u8], bit: u16) {
    let byte = bit as usize / 8;
    let mask = 1u8 << (bit % 8);
    if let Some(value) = buf.get_mut(byte) {
        *value |= mask;
    }
}

/// The [`AxisId`] the Linux layer assigns to a kernel ABS code.
///
/// The Linux layer identifies axes by their kernel ABS code: `ABS_MT_POSITION_X`
/// (53) becomes `AxisId::new(53)`, `ABS_MT_POSITION_Y` (54) becomes
/// `AxisId::new(54)`, and so on. The decoder looks up axis descriptions in
/// the device descriptor with this convention, and M4's device probing will
/// build descriptors the same way, so live and replayed descriptors agree.
#[must_use]
pub fn axis_id_for_code(code: u16) -> AxisId {
    AxisId::new(u32::from(code))
}
