//! Minimal Linux uinput Type-B touchpad fixture used only by system tests.

#![allow(unsafe_code)]

use std::ffi::CString;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use touchpad_linux::codes::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, BTN_LEFT, EV_ABS,
    EV_KEY, EV_SYN, INPUT_PROP_BUTTONPAD, INPUT_PROP_POINTER, SYN_REPORT,
};

const UINPUT_PATH: &str = "/dev/uinput";
const BUS_VIRTUAL: u16 = 0x06;

const fn iow(nr: u8, size: usize) -> libc::c_ulong {
    ((1u64 << 30) | ((size as u64) << 16) | (0x55u64 << 8) | nr as u64) as libc::c_ulong
}

const fn io(nr: u8) -> libc::c_ulong {
    ((0x55u64 << 8) | nr as u64) as libc::c_ulong
}

const UI_DEV_CREATE: libc::c_ulong = io(1);
const UI_DEV_DESTROY: libc::c_ulong = io(2);
const UI_DEV_SETUP: libc::c_ulong = iow(3, size_of::<UinputSetup>());
const UI_ABS_SETUP: libc::c_ulong = iow(4, size_of::<UinputAbsSetup>());
const UI_SET_EVBIT: libc::c_ulong = iow(100, size_of::<libc::c_int>());
const UI_SET_KEYBIT: libc::c_ulong = iow(101, size_of::<libc::c_int>());
const UI_SET_ABSBIT: libc::c_ulong = iow(103, size_of::<libc::c_int>());
const UI_SET_PROPBIT: libc::c_ulong = iow(110, size_of::<libc::c_int>());

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UinputSetup {
    id: InputId,
    name: [libc::c_char; 80],
    ff_effects_max: u32,
}

impl Default for UinputSetup {
    fn default() -> Self {
        Self {
            id: InputId::default(),
            name: [0; 80],
            ff_effects_max: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UinputAbsSetup {
    code: u16,
    _padding: u16,
    absinfo: InputAbsInfo,
}

/// A real kernel-backed virtual Type-B touchpad.
pub struct VirtualTouchpad {
    fd: OwnedFd,
    name: String,
    event_path: PathBuf,
}

impl VirtualTouchpad {
    /// Returns whether the host exposes `/dev/uinput`.
    #[must_use]
    pub fn available() -> bool {
        Path::new(UINPUT_PATH).exists()
    }

    /// Creates a five-slot 130x80 mm buttonpad with deterministic resolution.
    pub fn create() -> io::Result<Self> {
        let name = format!("touchpad2mac-system-test-{}", std::process::id());
        let path = CString::new(UINPUT_PATH).expect("static path has no NUL");
        let raw = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let raw_fd = fd.as_raw_fd();

        ioctl_int(raw_fd, UI_SET_EVBIT, i32::from(EV_KEY))?;
        ioctl_int(raw_fd, UI_SET_EVBIT, i32::from(EV_ABS))?;
        ioctl_int(raw_fd, UI_SET_KEYBIT, i32::from(BTN_LEFT))?;
        ioctl_int(raw_fd, UI_SET_PROPBIT, i32::from(INPUT_PROP_POINTER))?;
        ioctl_int(raw_fd, UI_SET_PROPBIT, i32::from(INPUT_PROP_BUTTONPAD))?;

        abs(raw_fd, ABS_MT_SLOT, 0, 4, 0)?;
        abs(raw_fd, ABS_MT_TRACKING_ID, 0, 65_535, 0)?;
        // 10 units/mm => 130x80 mm physical surface.
        abs(raw_fd, ABS_MT_POSITION_X, 0, 1300, 10)?;
        abs(raw_fd, ABS_MT_POSITION_Y, 0, 800, 10)?;

        let mut setup = UinputSetup {
            id: InputId {
                bustype: BUS_VIRTUAL,
                vendor: 0x1209,
                product: 0x2ac0,
                version: 1,
            },
            ..UinputSetup::default()
        };
        for (dst, src) in setup.name.iter_mut().zip(name.bytes()) {
            *dst = src as libc::c_char;
        }
        ioctl_ptr(raw_fd, UI_DEV_SETUP, &setup)?;
        ioctl_none(raw_fd, UI_DEV_CREATE)?;

        let event_path = wait_for_event_node(&name, Duration::from_secs(3))?;
        Ok(Self {
            fd,
            name,
            event_path,
        })
    }

    /// Kernel-visible device name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Corresponding `/dev/input/event*` node.
    #[must_use]
    pub fn event_path(&self) -> &Path {
        &self.event_path
    }

    /// Emits one complete three-contact Type-B frame.
    pub fn three_contacts(&self, x: i32, y: i32) -> io::Result<()> {
        for slot in 0..3 {
            self.emit(EV_ABS, ABS_MT_SLOT, slot)?;
            self.emit(EV_ABS, ABS_MT_TRACKING_ID, 100 + slot)?;
            self.emit(EV_ABS, ABS_MT_POSITION_X, x + slot * 50)?;
            self.emit(EV_ABS, ABS_MT_POSITION_Y, y)?;
        }
        self.emit(EV_SYN, SYN_REPORT, 0)
    }

    /// Releases the first three contacts in one Type-B frame.
    pub fn release_three(&self) -> io::Result<()> {
        for slot in 0..3 {
            self.emit(EV_ABS, ABS_MT_SLOT, slot)?;
            self.emit(EV_ABS, ABS_MT_TRACKING_ID, -1)?;
        }
        self.emit(EV_SYN, SYN_REPORT, 0)
    }

    fn emit(&self, event_type: u16, code: u16, value: i32) -> io::Result<()> {
        let event = libc::input_event {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_: event_type,
            code,
            value,
        };
        let rc = unsafe {
            libc::write(
                self.fd.as_raw_fd(),
                (&event as *const libc::input_event).cast(),
                size_of::<libc::input_event>(),
            )
        };
        if rc == size_of::<libc::input_event>() as isize {
            Ok(())
        } else if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short uinput write",
            ))
        }
    }
}

impl Drop for VirtualTouchpad {
    fn drop(&mut self) {
        let _ = unsafe { libc::ioctl(self.fd.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

fn abs(fd: i32, code: u16, min: i32, max: i32, resolution: i32) -> io::Result<()> {
    ioctl_int(fd, UI_SET_ABSBIT, i32::from(code))?;
    let setup = UinputAbsSetup {
        code,
        _padding: 0,
        absinfo: InputAbsInfo {
            minimum: min,
            maximum: max,
            resolution,
            ..InputAbsInfo::default()
        },
    };
    ioctl_ptr(fd, UI_ABS_SETUP, &setup)
}

fn ioctl_int(fd: i32, request: libc::c_ulong, value: i32) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd, request, value) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_ptr<T>(fd: i32, request: libc::c_ulong, value: &T) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd, request, value as *const T) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_none(fd: i32, request: libc::c_ulong) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd, request) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wait_for_event_node(name: &str, timeout: Duration) -> io::Result<PathBuf> {
    let deadline = Instant::now() + timeout;
    loop {
        for entry in fs::read_dir("/sys/class/input")? {
            let entry = entry?;
            let file_name = entry.file_name();
            let Some(event_name) = file_name.to_str() else {
                continue;
            };
            if !event_name.starts_with("event") {
                continue;
            }
            let device_name =
                fs::read_to_string(entry.path().join("device/name")).unwrap_or_default();
            if device_name.trim() == name {
                return Ok(PathBuf::from("/dev/input").join(event_name));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("uinput device {name:?} did not appear in /sys/class/input"),
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}
