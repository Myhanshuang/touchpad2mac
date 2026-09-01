//! Read-only keyboard discovery and anonymous typing-activity monitoring.
//!
//! The monitor never grabs a keyboard and never exposes key codes outside
//! this module. Raw `EV_KEY` data is reduced immediately to monotonic typing
//! timestamps suitable for core DWT.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use touchpad_core::Monotonic;

use crate::codes::{bits_to_bytes, test_bit, EV_KEY, EV_MAX, KEY_MAX};
use crate::event::{decode_input_events, EventDecodeError, TimevalError, INPUT_EVENT_SIZE};
use crate::sys::{Fd, InputId, Sys, SysError, CLOCK_MONOTONIC};

const BUS_I8042: u16 = 0x11;
const BUS_I2C: u16 = 0x18;
const BUS_HOST: u16 = 0x19;
const BUS_SPI: u16 = 0x1c;

const KEY_ESC: u16 = 1;
const KEY_1: u16 = 2;
const KEY_EQUAL: u16 = 13;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_Q: u16 = 16;
const KEY_RIGHTBRACE: u16 = 27;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_A: u16 = 30;
const KEY_GRAVE: u16 = 41;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_BACKSLASH: u16 = 43;
const KEY_Z: u16 = 44;
const KEY_SLASH: u16 = 53;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_SPACE: u16 = 57;
const KEY_CAPSLOCK: u16 = 58;
const KEY_102ND: u16 = 86;
const KEY_RO: u16 = 89;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;
const KEY_DELETE: u16 = 111;
const KEY_YEN: u16 = 124;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;
const KEY_COMPOSE: u16 = 127;
const KEY_FN: u16 = 0x1d0;

/// A keyboard device accepted as a DWT activity source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardCandidate {
    /// `/dev/input/event*` node.
    pub path: PathBuf,
    /// Kernel device name, used only for diagnostics.
    pub name: String,
    /// Kernel `input_id` used for touchpad/keyboard pairing.
    pub id: InputId,
    /// Whether this device is treated as integrated with the touchpad.
    pub internal: bool,
}

/// Failure while preparing or reading a read-only keyboard monitor.
#[derive(Debug, thiserror::Error)]
pub enum KeyboardError {
    /// `/dev/input` could not be enumerated.
    #[error("could not enumerate /dev/input for keyboards: {0}")]
    Enumerate(SysError),
    /// A selected keyboard could not be opened read-only.
    #[error("could not open keyboard {path}: {source}")]
    Open {
        /// Device path.
        path: PathBuf,
        /// Underlying syscall failure.
        source: SysError,
    },
    /// Kernel monotonic timestamps could not be selected.
    #[error("could not set CLOCK_MONOTONIC on keyboard {path}: {source}")]
    Clock {
        /// Device path.
        path: PathBuf,
        /// Underlying ioctl failure.
        source: SysError,
    },
    /// A keyboard read failed.
    #[error("could not read keyboard {path}: {source}")]
    Read {
        /// Device path.
        path: PathBuf,
        /// Underlying read failure.
        source: SysError,
    },
    /// The keyboard returned EOF (typically unplug/removal).
    #[error("keyboard {path} disappeared")]
    Gone {
        /// Device path.
        path: PathBuf,
    },
    /// Raw bytes were not a valid sequence of `input_event`s.
    #[error("invalid keyboard input-event buffer from {path}: {source}")]
    Decode {
        /// Device path.
        path: PathBuf,
        /// Decode failure.
        source: EventDecodeError,
    },
    /// A keyboard event carried an invalid monotonic timestamp.
    #[error("invalid keyboard timestamp from {path}: {source}")]
    Timestamp {
        /// Device path.
        path: PathBuf,
        /// Timestamp conversion failure.
        source: TimevalError,
    },
}

/// Discovers keyboard-like event nodes suitable for DWT.
///
/// The touchpad node itself is excluded. A keyboard must either use an
/// integrated bus or share a non-zero
/// vendor/product identity with the touchpad, mirroring libinput's pairing
/// principle without relying on device-name strings.
pub fn discover_keyboards(
    sys: &dyn Sys,
    touchpad_path: &Path,
    touchpad_id: InputId,
) -> Result<Vec<KeyboardCandidate>, KeyboardError> {
    let mut paths = sys
        .read_dir(Path::new("/dev/input"))
        .map_err(KeyboardError::Enumerate)?;
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        if path == touchpad_path || !crate::device::is_event_node(&path) {
            continue;
        }
        let Ok(fd) = sys.open(&path) else {
            continue;
        };
        let probed = probe_keyboard_fd(sys, fd, &path, touchpad_id);
        let _ = sys.close(fd);
        if let Ok(Some(candidate)) = probed {
            out.push(candidate);
        }
    }
    Ok(out)
}

fn probe_keyboard_fd(
    sys: &dyn Sys,
    fd: Fd,
    path: &Path,
    touchpad_id: InputId,
) -> Result<Option<KeyboardCandidate>, SysError> {
    let mut ev_bits = vec![0; bits_to_bytes(EV_MAX)];
    let ev_n = sys.ioctl_ev_bits(fd, 0, &mut ev_bits)?;
    if ev_n == 0 || !test_bit(&ev_bits, EV_KEY) {
        return Ok(None);
    }
    let mut key_bits = vec![0; bits_to_bytes(KEY_MAX)];
    let key_n = sys.ioctl_ev_bits(fd, EV_KEY, &mut key_bits)?;
    if key_n == 0 || !looks_like_typing_keyboard(&key_bits) {
        return Ok(None);
    }
    let id = sys.ioctl_id(fd)?;
    let internal = is_internal_bus(id.bustype) || matching_identity(id, touchpad_id);
    // Like libinput's built-in touchpad DWT pairing, external keyboards do
    // not disable the integrated touchpad. This is deliberately a device
    // policy, not a hot-reloadable feel setting.
    if !internal {
        return Ok(None);
    }
    let mut name_buf = [0u8; 256];
    let n = sys.ioctl_name(fd, &mut name_buf)?;
    let name = String::from_utf8_lossy(&name_buf[..n.min(name_buf.len())])
        .trim_end_matches('\0')
        .to_string();
    Ok(Some(KeyboardCandidate {
        path: path.to_path_buf(),
        name,
        id,
        internal,
    }))
}

fn looks_like_typing_keyboard(bits: &[u8]) -> bool {
    [KEY_Q, KEY_A, KEY_Z, KEY_SPACE, KEY_ENTER]
        .into_iter()
        .all(|key| test_bit(bits, key))
}

fn is_internal_bus(bus: u16) -> bool {
    matches!(bus, BUS_I8042 | BUS_I2C | BUS_HOST | BUS_SPI)
}

fn matching_identity(keyboard: InputId, touchpad: InputId) -> bool {
    keyboard.vendor != 0
        && keyboard.product != 0
        && keyboard.vendor == touchpad.vendor
        && keyboard.product == touchpad.product
}

/// Open read-only keyboard activity source.
pub struct KeyboardMonitor {
    sys: Rc<dyn Sys>,
    path: PathBuf,
    fd: Option<Fd>,
    pressed: BTreeSet<u16>,
}

impl KeyboardMonitor {
    /// Opens the candidate read-only and switches its event clock to
    /// `CLOCK_MONOTONIC`. This method never issues `EVIOCGRAB`.
    pub fn open(sys: Rc<dyn Sys>, candidate: &KeyboardCandidate) -> Result<Self, KeyboardError> {
        let fd = sys
            .open(&candidate.path)
            .map_err(|source| KeyboardError::Open {
                path: candidate.path.clone(),
                source,
            })?;
        if let Err(source) = sys.ioctl_set_clock_id(fd, CLOCK_MONOTONIC) {
            let _ = sys.close(fd);
            return Err(KeyboardError::Clock {
                path: candidate.path.clone(),
                source,
            });
        }
        Ok(Self {
            sys,
            path: candidate.path.clone(),
            fd: Some(fd),
            pressed: BTreeSet::new(),
        })
    }

    #[must_use]
    /// The current read-only fd, or `None` after close.
    pub const fn fd(&self) -> Option<Fd> {
        self.fd
    }

    /// Drains all currently-ready keyboard input and returns only anonymous
    /// typing timestamps. Key codes are consumed locally and never escape.
    pub fn read_activity(&mut self) -> Result<Vec<Monotonic>, KeyboardError> {
        let fd = self.fd.expect("open keyboard monitor");
        let mut activity = Vec::new();
        loop {
            let mut buf = [0u8; INPUT_EVENT_SIZE * 64];
            let n = self
                .sys
                .read(fd, &mut buf)
                .map_err(|source| KeyboardError::Read {
                    path: self.path.clone(),
                    source,
                })?;
            if n == 0 {
                return Err(KeyboardError::Gone {
                    path: self.path.clone(),
                });
            }
            let events =
                decode_input_events(&buf[..n]).map_err(|source| KeyboardError::Decode {
                    path: self.path.clone(),
                    source,
                })?;
            for event in events {
                if event.event_type != EV_KEY {
                    continue;
                }
                match event.value {
                    0 => {
                        self.pressed.remove(&event.code);
                    }
                    1 => {
                        if self.pressed.insert(event.code) && triggers_dwt(event.code) {
                            activity.push(event.to_monotonic().map_err(|source| {
                                KeyboardError::Timestamp {
                                    path: self.path.clone(),
                                    source,
                                }
                            })?);
                        }
                    }
                    2 => {}
                    _ => {}
                }
            }
            match self.sys.poll(fd, std::time::Duration::ZERO) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(source) => {
                    return Err(KeyboardError::Read {
                        path: self.path.clone(),
                        source,
                    })
                }
            }
        }
        Ok(activity)
    }

    /// Closes the read-only keyboard fd. Idempotent.
    pub fn close(&mut self) -> Result<(), SysError> {
        if let Some(fd) = self.fd.take() {
            self.sys.close(fd)?;
        }
        Ok(())
    }
}

impl Drop for KeyboardMonitor {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn triggers_dwt(key: u16) -> bool {
    if is_modifier(key) {
        return false;
    }
    matches!(
        key,
        KEY_ESC
            | KEY_BACKSPACE
            | KEY_TAB
            | KEY_ENTER
            | KEY_SPACE
            | KEY_DELETE
            | KEY_102ND
            | KEY_RO
            | KEY_YEN
    ) || (KEY_1..=KEY_EQUAL).contains(&key)
        || (KEY_Q..=KEY_RIGHTBRACE).contains(&key)
        || (KEY_A..=KEY_GRAVE).contains(&key)
        || key == KEY_BACKSLASH
        || (KEY_Z..=KEY_SLASH).contains(&key)
}

fn is_modifier(key: u16) -> bool {
    matches!(
        key,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_CAPSLOCK
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
            | KEY_COMPOSE
            | KEY_FN
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::set_bit;
    use crate::sys::mock::{MockCall, MockDevice, MockSys};

    const TEST_BUS_USB: u16 = 0x03;

    fn keyboard(name: &str, bus: u16) -> MockDevice {
        let mut device = MockDevice::new(name);
        device.id.bustype = bus;
        for key in [KEY_Q, KEY_A, KEY_Z, KEY_SPACE, KEY_ENTER] {
            device.add_key(key);
        }
        device
    }

    #[test]
    fn discovery_prefers_internal_typing_devices_and_not_hotkeys() {
        let sys = MockSys::new();
        let internal = PathBuf::from("/dev/input/event1");
        let external = PathBuf::from("/dev/input/event2");
        let hotkeys = PathBuf::from("/dev/input/event3");
        sys.set_dir_entries(vec![external.clone(), hotkeys.clone(), internal.clone()]);
        sys.add_device(&internal, keyboard("AT keyboard", BUS_I8042));
        sys.add_device(&external, keyboard("USB keyboard", TEST_BUS_USB));
        let mut hotkey = MockDevice::new("hotkeys");
        set_bit(&mut hotkey.ev_bits, EV_KEY);
        hotkey.add_key(KEY_SPACE);
        sys.add_device(&hotkeys, hotkey);
        let found =
            discover_keyboards(&sys, Path::new("/dev/input/event0"), InputId::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, internal);
    }

    #[test]
    fn monitor_never_grabs_and_filters_modifiers_and_repeat() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event1");
        let mut dev = keyboard("AT keyboard", BUS_I8042);
        dev.push_event(1, 0, EV_KEY, KEY_LEFTCTRL, 1);
        dev.push_event(1, 1000, EV_KEY, KEY_A, 1);
        dev.push_event(1, 2000, EV_KEY, KEY_A, 2);
        dev.push_event(1, 3000, EV_KEY, KEY_A, 0);
        sys.add_device(&path, dev);
        let candidate = KeyboardCandidate {
            path,
            name: "AT keyboard".into(),
            id: InputId {
                bustype: BUS_I8042,
                ..InputId::default()
            },
            internal: true,
        };
        let sys_dyn: Rc<dyn Sys> = sys.clone();
        let mut monitor = KeyboardMonitor::open(sys_dyn, &candidate).unwrap();
        let activity = monitor.read_activity().unwrap();
        assert_eq!(activity, vec![Monotonic::from_nanos(1_001_000_000)]);
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    }
}
