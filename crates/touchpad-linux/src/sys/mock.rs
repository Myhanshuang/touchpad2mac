//! Programmable [`Sys`] test double (M4).
//!
//! [`MockSys`] backs every test in the crate (and downstream integration
//! tests): it serves directory listings, device nodes with configurable
//! capabilities and ioctl answers, a scripted raw-event stream, and a
//! complete call log, so tests can assert exactly which syscalls happened
//! (e.g. "the grab was released exactly once, before the fd was closed")
//! without ever opening or grabbing a real device.
//!
//! This module is `unsafe`-free and platform-independent.

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use super::{AbsInfo, Fd, InputId, Sys, SysError};
use crate::codes::{
    bits_to_bytes, set_bit, ABS_MAX, EV_ABS, EV_KEY, EV_MAX, INPUT_PROP_MAX, KEY_MAX,
};

/// A programmable failure kind the mock can inject (converted to
/// [`SysError`] on return). `Copy` so scripts can be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockFailure {
    /// Maps to `ENOENT` ([`SysError::NotFound`]).
    NotFound,
    /// Maps to `EACCES`/`EPERM` ([`SysError::PermissionDenied`]).
    PermissionDenied,
    /// Maps to `EINTR` ([`SysError::Interrupted`]).
    Interrupted,
    /// Maps to a generic I/O error ([`SysError::Io`]).
    Io,
}

/// Converts a scripted failure into a structured error, attaching `path`
/// where the variant carries one.
#[must_use]
pub fn failure_to_sys_error(failure: MockFailure, path: &Path) -> SysError {
    match failure {
        MockFailure::NotFound => SysError::NotFound {
            path: path.to_path_buf(),
        },
        MockFailure::PermissionDenied => SysError::PermissionDenied {
            path: path.to_path_buf(),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        },
        MockFailure::Interrupted => SysError::Interrupted,
        MockFailure::Io => SysError::Io(io::Error::from(io::ErrorKind::Other)),
    }
}

/// One scripted step of a mock device's raw `read` stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadChunk {
    /// Return these raw bytes (may be a torn, non-24-byte chunk to test
    /// partial reads).
    Bytes(Vec<u8>),
    /// Fail the read with this error (e.g. `EINTR`).
    Failure(MockFailure),
    /// Return EOF (device unplugged).
    Eof,
}

/// A fake evdev device served by [`MockSys`].
///
/// Fields mirror what a kernel evdev node reports: name, id, capability bit
/// arrays, per-axis `input_absinfo`, per-axis per-slot MT values, key state,
/// grab state, and a scripted read stream. When the read stream is empty the
/// mock returns EOF, which models an unplugged device.
#[derive(Debug)]
pub struct MockDevice {
    /// `EVIOCGNAME` result.
    pub name: String,
    /// `EVIOCGID` result.
    pub id: InputId,
    /// `EVIOCGBIT(0, len)` result: the `EV_*` bit array (4 bytes).
    pub ev_bits: Vec<u8>,
    /// `EVIOCGBIT(EV_KEY, len)` result (96 bytes).
    pub key_bits: Vec<u8>,
    /// `EVIOCGBIT(EV_ABS, len)` result (8 bytes).
    pub abs_bits: Vec<u8>,
    /// `EVIOCGPROP(len)` result (4 bytes).
    pub prop_bits: Vec<u8>,
    /// `EVIOCGABS` results per ABS code.
    pub absinfo: BTreeMap<u16, AbsInfo>,
    /// `EVIOCGMTSLOTS` result per ABS code (each entry holds the device's
    /// own `num_slots` values; `evdev_handle_mt_request` copies at most
    /// `buf.len() - 1` of them after the leading code and returns 0 — the
    /// mock mirrors that truncation and never invents a size-mismatch
    /// error, M4 review RR2).
    pub mt_slots: BTreeMap<u16, Vec<i32>>,
    /// `EVIOCGKEY` result (96 bytes).
    pub key_state: Vec<u8>,
    /// Current grab state (`EVIOCGRAB`).
    pub grab: bool,
    /// Scripted `read` stream, consumed front to back; empty means EOF.
    pub reads: VecDeque<ReadChunk>,
    /// When set, every `EVIOCGMTSLOTS` call fails with this error (models a
    /// resync ioctl failure).
    pub mt_slots_error: Option<MockFailure>,
    /// When set, `EVIOCSCLOCKID` fails with this error (models a clock
    /// setup failure).
    pub clock_id_error: Option<MockFailure>,
    /// When set, `EVIOCGRAB(true)` (grab) fails with this error while a
    /// successful release still works (models a failed grab).
    pub grab_error: Option<MockFailure>,
    /// When set, `EVIOCGRAB(false)` (release) fails with this error while a
    /// successful grab still works (models a failed ungrab).
    pub release_error: Option<MockFailure>,
    /// When set, every ioctl fails with this error (models a general ioctl
    /// failure).
    pub ioctl_error: Option<MockFailure>,
    /// When set, `close` fails with this error (models a failed close during
    /// cleanup, M5 review R3 fault injection). The fd is still removed from
    /// the registry so the close remains idempotent.
    pub close_error: Option<MockFailure>,
    /// When set, `poll` reports the device as hung up (`POLLHUP` → ready)
    /// even with an empty read stream, modelling an unplugged device whose fd
    /// still reports hangup so the takeover loop reads and surfaces the EOF
    /// instead of idling until the deadline (M10 review R2).
    pub poll_hup: bool,
    /// When set, `poll` reports `POLLNVAL` (an immediate structured error),
    /// modelling an invalid fd (M10 review R2).
    pub poll_nval: bool,
    /// When set, the poll call itself fails with this error (modelling a
    /// poll(2) failure distinct from the fd state, e.g. EINTR).
    pub poll_error: Option<MockFailure>,
}

impl MockDevice {
    /// Creates a device with the given name and no capabilities.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: InputId::default(),
            ev_bits: vec![0; bits_to_bytes(EV_MAX)],
            key_bits: vec![0; bits_to_bytes(KEY_MAX)],
            abs_bits: vec![0; bits_to_bytes(ABS_MAX)],
            prop_bits: vec![0; bits_to_bytes(INPUT_PROP_MAX)],
            absinfo: BTreeMap::new(),
            mt_slots: BTreeMap::new(),
            key_state: vec![0; bits_to_bytes(KEY_MAX)],
            grab: false,
            reads: VecDeque::new(),
            mt_slots_error: None,
            clock_id_error: None,
            grab_error: None,
            release_error: None,
            ioctl_error: None,
            close_error: None,
            poll_hup: false,
            poll_nval: false,
            poll_error: None,
        }
    }

    /// Builds a complete Type-B touchpad candidate: reports `EV_KEY` +
    /// `EV_ABS`, all four required MT axes (`ABS_MT_SLOT`,
    /// `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X/Y`), `INPUT_PROP_POINTER`,
    /// X/Y/SLOT `absinfo` at the given resolution, and `slot_count` slots.
    #[must_use]
    pub fn touchpad(name: impl Into<String>, slot_count: u32) -> Self {
        let mut device = Self::new(name);
        set_bit(&mut device.ev_bits, EV_KEY);
        set_bit(&mut device.ev_bits, EV_ABS);
        for (code, max) in [
            (crate::ABS_MT_SLOT, slot_count.saturating_sub(1) as i32),
            (crate::ABS_MT_TRACKING_ID, 65535),
            (crate::ABS_MT_POSITION_X, 3000),
            (crate::ABS_MT_POSITION_Y, 2000),
        ] {
            set_bit(&mut device.abs_bits, code);
            device.absinfo.insert(
                code,
                AbsInfo {
                    value: 0,
                    min: 0,
                    max,
                    fuzz: 0,
                    flat: 0,
                    resolution: 100,
                },
            );
        }
        set_bit(&mut device.prop_bits, crate::INPUT_PROP_POINTER);
        device
            .mt_slots
            .insert(crate::ABS_MT_TRACKING_ID, vec![-1; slot_count as usize]);
        device
            .mt_slots
            .insert(crate::ABS_MT_POSITION_X, vec![0; slot_count as usize]);
        device
            .mt_slots
            .insert(crate::ABS_MT_POSITION_Y, vec![0; slot_count as usize]);
        device
    }

    /// Marks a key code as reported (`EV_KEY` bit array + `keybit`).
    pub fn add_key(&mut self, code: u16) {
        set_bit(&mut self.ev_bits, EV_KEY);
        set_bit(&mut self.key_bits, code);
    }

    /// Marks an ABS code as reported and records its `absinfo`.
    pub fn add_abs(&mut self, code: u16, info: AbsInfo) {
        set_bit(&mut self.ev_bits, EV_ABS);
        set_bit(&mut self.abs_bits, code);
        self.absinfo.insert(code, info);
    }

    /// Marks an input property as set.
    pub fn add_prop(&mut self, prop: u16) {
        set_bit(&mut self.prop_bits, prop);
    }

    /// Records the per-slot values `EVIOCGMTSLOTS` returns for `code`.
    pub fn set_mt_slots(&mut self, code: u16, values: Vec<i32>) {
        self.mt_slots.insert(code, values);
    }

    /// Sets the current state of a key code (`EVIOCGKEY` result).
    pub fn set_key_state(&mut self, code: u16, pressed: bool) {
        let byte = code as usize / 8;
        let mask = 1u8 << (code % 8);
        if pressed {
            self.key_state[byte] |= mask;
        } else {
            self.key_state[byte] &= !mask;
        }
    }

    /// Appends one raw kernel-style `input_event` (the current target's
    /// layout) to the read stream with the given monotonic `(sec, usec)`
    /// timeval.
    pub fn push_event(&mut self, sec: i64, usec: i64, event_type: u16, code: u16, value: i32) {
        self.reads
            .push_back(ReadChunk::Bytes(crate::event::encode_input_event(
                sec, usec, event_type, code, value,
            )));
    }

    /// Appends a raw (possibly torn) byte chunk to the read stream.
    pub fn push_raw(&mut self, bytes: Vec<u8>) {
        self.reads.push_back(ReadChunk::Bytes(bytes));
    }

    /// Appends a scripted read failure.
    pub fn push_read_failure(&mut self, failure: MockFailure) {
        self.reads.push_back(ReadChunk::Failure(failure));
    }

    /// Appends an explicit EOF chunk.
    pub fn push_eof(&mut self) {
        self.reads.push_back(ReadChunk::Eof);
    }
}

/// A recorded syscall for test assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockCall {
    /// `read_dir` on the given path.
    ReadDir(PathBuf),
    /// `open` of the given path, returning the given handle.
    Open(PathBuf),
    /// `close` of the given handle.
    Close(Fd),
    /// `read` on the handle, returning `n` bytes.
    Read(Fd, usize),
    /// `EVIOCGRAB` with the given grab state.
    Grab(Fd, bool),
    /// `EVIOCGNAME`.
    Name(Fd),
    /// `EVIOCGID`.
    Id(Fd),
    /// `EVIOCGBIT` for the given event type.
    EvBits(Fd, u16),
    /// `EVIOCGPROP`.
    PropBits(Fd),
    /// `EVIOCGKEY`.
    KeyState(Fd),
    /// `EVIOCSCLOCKID` for the given clock id.
    ClockId(Fd, u32),
    /// `EVIOCGABS` for the given ABS code.
    AbsInfo(Fd, u16),
    /// `EVIOCGMTSLOTS` for the given ABS code.
    MtSlots(Fd, u16),
}

/// The programmable [`Sys`] test double.
#[derive(Default)]
pub struct MockSys {
    dir_entries: RefCell<Vec<PathBuf>>,
    read_dir_error: RefCell<Option<MockFailure>>,
    devices: RefCell<HashMap<PathBuf, Rc<RefCell<MockDevice>>>>,
    open_errors: RefCell<HashMap<PathBuf, MockFailure>>,
    next_fd: Cell<u64>,
    open_fds: RefCell<HashMap<u64, PathBuf>>,
    log: RefCell<Vec<MockCall>>,
}

impl MockSys {
    /// Creates an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the directory listing returned by [`Sys::read_dir`].
    pub fn set_dir_entries(&self, entries: Vec<PathBuf>) {
        *self.dir_entries.borrow_mut() = entries;
    }

    /// Configures `read_dir` to fail with the given error.
    pub fn set_read_dir_error(&self, failure: MockFailure) {
        *self.read_dir_error.borrow_mut() = Some(failure);
    }

    /// Registers a device node that `open` will serve.
    pub fn add_device(&self, path: impl Into<PathBuf>, device: MockDevice) {
        self.devices
            .borrow_mut()
            .insert(path.into(), Rc::new(RefCell::new(device)));
    }

    /// Registers an `open` failure for a path (takes precedence over a
    /// registered device).
    pub fn set_open_error(&self, path: impl Into<PathBuf>, failure: MockFailure) {
        self.open_errors.borrow_mut().insert(path.into(), failure);
    }

    /// Returns the registered device for `path`, for direct mutation.
    #[must_use]
    pub fn device(&self, path: &Path) -> Option<Rc<RefCell<MockDevice>>> {
        self.devices.borrow().get(path).cloned()
    }

    /// The syscall log so far, in call order.
    #[must_use]
    pub fn log(&self) -> Vec<MockCall> {
        self.log.borrow().clone()
    }

    /// Counts how many times a call matching `predicate` occurred.
    #[must_use]
    pub fn count(&self, predicate: impl Fn(&MockCall) -> bool) -> usize {
        self.log
            .borrow()
            .iter()
            .filter(|call| predicate(call))
            .count()
    }

    fn device_for(&self, fd: Fd) -> Result<Rc<RefCell<MockDevice>>, SysError> {
        let path = self
            .open_fds
            .borrow()
            .get(&fd.as_u64())
            .cloned()
            .ok_or(SysError::Closed(fd))?;
        let device = self
            .devices
            .borrow()
            .get(&path)
            .cloned()
            .ok_or(SysError::Closed(fd))?;
        Ok(device)
    }
}

impl Sys for MockSys {
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, SysError> {
        self.log
            .borrow_mut()
            .push(MockCall::ReadDir(path.to_path_buf()));
        if let Some(failure) = self.read_dir_error.borrow().as_ref() {
            return Err(failure_to_sys_error(*failure, path));
        }
        Ok(self.dir_entries.borrow().clone())
    }

    fn open(&self, path: &Path) -> Result<Fd, SysError> {
        if let Some(failure) = self.open_errors.borrow().get(path) {
            return Err(failure_to_sys_error(*failure, path));
        }
        if !self.devices.borrow().contains_key(path) {
            return Err(SysError::NotFound {
                path: path.to_path_buf(),
            });
        }
        let fd = Fd::new(self.next_fd.get());
        self.next_fd.set(self.next_fd.get() + 1);
        self.open_fds
            .borrow_mut()
            .insert(fd.as_u64(), path.to_path_buf());
        self.log
            .borrow_mut()
            .push(MockCall::Open(path.to_path_buf()));
        Ok(fd)
    }

    fn close(&self, fd: Fd) -> Result<(), SysError> {
        let removed = self.open_fds.borrow_mut().remove(&fd.as_u64());
        if removed.is_none() {
            // Idempotent: closing an already-closed handle succeeds.
            return Ok(());
        }
        // M5 review R3 fault injection: a device may be configured to fail
        // `close`. The fd is still removed from the registry, so the close
        // stays idempotent and the failure is reported exactly once.
        let path = removed.as_ref().expect("just removed");
        if let Some(device) = self.devices.borrow().get(path) {
            if let Some(failure) = device.borrow().close_error {
                self.log.borrow_mut().push(MockCall::Close(fd));
                return Err(failure_to_sys_error(failure, path));
            }
        }
        self.log.borrow_mut().push(MockCall::Close(fd));
        Ok(())
    }

    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "read buffer is empty".to_string(),
            ));
        }
        let device = self.device_for(fd)?;
        let mut dev = device.borrow_mut();
        match dev.reads.pop_front() {
            Some(ReadChunk::Bytes(bytes)) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                self.log.borrow_mut().push(MockCall::Read(fd, n));
                Ok(n)
            }
            Some(ReadChunk::Failure(failure)) => {
                self.log.borrow_mut().push(MockCall::Read(fd, 0));
                Err(failure_to_sys_error(failure, Path::new("")))
            }
            Some(ReadChunk::Eof) | None => {
                self.log.borrow_mut().push(MockCall::Read(fd, 0));
                Ok(0)
            }
        }
    }

    fn ioctl_set_clock_id(&self, fd: Fd, clock_id: u32) -> Result<(), SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.clock_id_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::ClockId(fd, clock_id));
        Ok(())
    }

    fn ioctl_grab(&self, fd: Fd, grab: bool) -> Result<(), SysError> {
        let device = self.device_for(fd)?;
        let mut dev = device.borrow_mut();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        if grab {
            if let Some(failure) = dev.grab_error {
                self.log.borrow_mut().push(MockCall::Grab(fd, true));
                return Err(failure_to_sys_error(failure, Path::new("")));
            }
        } else if let Some(failure) = dev.release_error {
            // The release attempt is still recorded: a failed EVIOCGRAB(0)
            // is observable and leaves the grab held.
            self.log.borrow_mut().push(MockCall::Grab(fd, false));
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        dev.grab = grab;
        self.log.borrow_mut().push(MockCall::Grab(fd, grab));
        Ok(())
    }

    fn ioctl_name(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::Name(fd));
        let name = dev.name.as_bytes();
        let n = name.len().min(buf.len());
        buf[..n].copy_from_slice(&name[..n]);
        Ok(n)
    }

    fn ioctl_id(&self, fd: Fd) -> Result<InputId, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::Id(fd));
        Ok(dev.id)
    }

    fn ioctl_ev_bits(&self, fd: Fd, ev_type: u16, buf: &mut [u8]) -> Result<usize, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::EvBits(fd, ev_type));
        let bits = match ev_type {
            0 => &dev.ev_bits,
            EV_KEY => &dev.key_bits,
            EV_ABS => &dev.abs_bits,
            _ => &[][..],
        };
        let n = bits.len().min(buf.len());
        buf[..n].copy_from_slice(&bits[..n]);
        Ok(n)
    }

    fn ioctl_prop_bits(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::PropBits(fd));
        let n = dev.prop_bits.len().min(buf.len());
        buf[..n].copy_from_slice(&dev.prop_bits[..n]);
        Ok(n)
    }

    fn ioctl_key_state(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::KeyState(fd));
        let n = dev.key_state.len().min(buf.len());
        buf[..n].copy_from_slice(&dev.key_state[..n]);
        Ok(n)
    }

    fn ioctl_absinfo(&self, fd: Fd, abs_code: u16) -> Result<AbsInfo, SysError> {
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.ioctl_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        self.log.borrow_mut().push(MockCall::AbsInfo(fd, abs_code));
        // The kernel reports zeroed absinfo for unreported axes.
        Ok(dev.absinfo.get(&abs_code).copied().unwrap_or_default())
    }

    fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError> {
        if buf.is_empty() {
            return Err(SysError::InvalidArgument(
                "MT slot buffer is empty".to_string(),
            ));
        }
        let device = self.device_for(fd)?;
        let values = {
            let dev = device.borrow_mut();
            if let Some(failure) = dev.mt_slots_error {
                return Err(failure_to_sys_error(failure, Path::new("")));
            }
            if let Some(failure) = dev.ioctl_error {
                return Err(failure_to_sys_error(failure, Path::new("")));
            }
            let code = buf[0] as u16;
            self.log.borrow_mut().push(MockCall::MtSlots(fd, code));
            dev.mt_slots.get(&code).cloned().ok_or_else(|| {
                // Mirrors the kernel's -ENOENT for an axis the device does not
                // report through the MT protocol.
                SysError::Io(io::Error::from(io::ErrorKind::NotFound))
            })?
        };
        // Mirror the kernel's `evdev_handle_mt_request`: it computes
        // `max_slots = (size - sizeof(__u32)) / sizeof(__s32)`, writes
        // `min(mt->num_slots, max_slots)` values after the leading code and
        // returns 0. It performs **no** size-mismatch rejection, so values
        // that do not fit the buffer are truncated, not errored (M4 review
        // RR2 — a regression against the old invented `-EINVAL`).
        let copy = values.len().min(buf.len().saturating_sub(1));
        buf[1..=copy].copy_from_slice(&values[..copy]);
        Ok(())
    }

    fn poll(&self, fd: Fd, _timeout: Duration) -> Result<bool, SysError> {
        // The mock models the explicit revents classification of the real
        // poll(2) seam (M10 review R2): a scripted poll failure surfaces
        // first, then `POLLNVAL` (an invalid fd — an immediate structured
        // error, never idle), then `POLLHUP` (an unplugged device — ready,
        // so the loop reads and surfaces the EOF), and only then "readiness
        // follows the scripted read stream": a read would not block (returns
        // bytes or a scripted failure) while chunks are pending, and an
        // exhausted stream is an idle device (never ready), so the takeover
        // loop's deadline still expires. An explicit `ReadChunk::Eof` is a
        // pending chunk, so the loop reads it and observes the unplug
        // (EOF → `DeviceGone`). The takeover CLI tests drive a scripted
        // readiness closure instead of this method; this faithful
        // implementation keeps the seam honest for any direct user.
        let device = self.device_for(fd)?;
        let dev = device.borrow();
        if let Some(failure) = dev.poll_error {
            return Err(failure_to_sys_error(failure, Path::new("")));
        }
        if dev.poll_nval {
            return Err(SysError::InvalidArgument(
                "poll: the fd is invalid (POLLNVAL); no read can make progress".to_string(),
            ));
        }
        if dev.poll_hup {
            return Ok(true);
        }
        let reads_pending = !dev.reads.is_empty();
        Ok(reads_pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev_bytes(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
        crate::event::encode_input_event(sec, usec, event_type, code, value)
    }

    #[test]
    fn open_close_read_round_trip() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::new("mock"));
        let fd = sys.open(&path).unwrap();
        let device = sys.device(&path).unwrap();
        device.borrow_mut().push_event(0, 0, 3, 53, 100);
        let mut buf = [0u8; crate::event::INPUT_EVENT_SIZE];
        let n = sys.read(fd, &mut buf).unwrap();
        assert_eq!(n, crate::event::INPUT_EVENT_SIZE);
        assert_eq!(&buf[..], ev_bytes(0, 0, 3, 53, 100));
        // Exhausted stream -> EOF.
        assert_eq!(sys.read(fd, &mut buf).unwrap(), 0);
        // Close is idempotent and the handle is closed afterwards.
        sys.close(fd).unwrap();
        sys.close(fd).unwrap();
        assert!(matches!(sys.read(fd, &mut buf), Err(SysError::Closed(_))));
    }

    #[test]
    fn read_dir_serves_entries_or_failure() {
        let sys = MockSys::new();
        sys.set_dir_entries(vec![PathBuf::from("/dev/input/event0")]);
        assert_eq!(
            sys.read_dir(Path::new("/dev/input")).unwrap(),
            vec![PathBuf::from("/dev/input/event0")]
        );
        sys.set_read_dir_error(MockFailure::NotFound);
        assert!(matches!(
            sys.read_dir(Path::new("/dev/input")),
            Err(SysError::NotFound { .. })
        ));
    }

    #[test]
    fn open_errors_map_to_structured_variants() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.set_open_error(&path, MockFailure::PermissionDenied);
        assert!(matches!(
            sys.open(&path),
            Err(SysError::PermissionDenied { .. })
        ));
        sys.set_open_error(&path, MockFailure::Interrupted);
        assert!(matches!(sys.open(&path), Err(SysError::Interrupted)));
    }

    #[test]
    fn ioctl_mt_slots_returns_values_after_the_leading_code() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::touchpad("pad", 4));
        let fd = sys.open(&path).unwrap();
        let mut buf = [0i32; 5];
        buf[0] = crate::ABS_MT_TRACKING_ID as i32;
        // The kernel returns 0 on success (no byte count), so the mock's
        // success is `Ok(())` while `buf[1..]` carries one value per slot
        // (M4 review R2 — a regression against the old byte-count seam).
        sys.ioctl_mt_slots(fd, &mut buf).unwrap();
        assert_eq!(&buf[1..5], &[-1, -1, -1, -1]);
    }

    /// M4 review RR1 contract test: `EVIOCGKEY` carries its **copied byte
    /// count** as the success payload (kernel `evdev_handle_get_val` →
    /// `bits_to_user` returns the number of bytes copied, never 0), while
    /// `EVIOCGMTSLOTS` succeeds with the **unit value** (kernel
    /// `evdev_handle_mt_request` returns 0 and the per-slot values are the
    /// payload in `buf[1..]`). The two evdev queries must never be modeled
    /// with the same success shape.
    #[test]
    fn key_state_returns_copied_bytes_while_mt_slots_returns_unit() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("pad", 2);
        device.set_key_state(crate::BTN_LEFT, true);
        sys.add_device(&path, device);
        let fd = sys.open(&path).unwrap();

        // EVIOCGKEY: success payload is the number of bytes copied; the
        // snapshot layer validates `returned >= required` against it (R7).
        let mut key_buf = [0u8; bits_to_bytes(KEY_MAX)];
        let returned: usize = sys.ioctl_key_state(fd, &mut key_buf).unwrap();
        assert_eq!(returned, bits_to_bytes(KEY_MAX));
        assert!(crate::codes::test_bit(&key_buf, crate::BTN_LEFT));

        // EVIOCGMTSLOTS: success payload is `()` — the values are the
        // payload, in `buf[1..]` (R2/RR2).
        let mut slot_buf = [0i32; 3];
        slot_buf[0] = crate::ABS_MT_TRACKING_ID as i32;
        let _: () = sys.ioctl_mt_slots(fd, &mut slot_buf).unwrap();
        assert_eq!(&slot_buf[1..], &[-1, -1]);
    }

    #[test]
    fn clock_id_is_recorded_in_the_log() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::new("mock"));
        let fd = sys.open(&path).unwrap();
        sys.ioctl_set_clock_id(fd, crate::sys::CLOCK_MONOTONIC)
            .unwrap();
        assert_eq!(
            sys.count(|call| matches!(
                call,
                MockCall::ClockId(f, id) if *f == fd && *id == crate::sys::CLOCK_MONOTONIC
            )),
            1
        );
    }

    #[test]
    fn grab_is_recorded_in_the_log() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::new("mock"));
        let fd = sys.open(&path).unwrap();
        sys.ioctl_grab(fd, true).unwrap();
        sys.ioctl_grab(fd, false).unwrap();
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
    }

    /// M10 review R2: the mock's `poll` models the explicit revents
    /// classification — pending reads are ready, an exhausted stream is idle,
    /// a hung-up device is ready (so the loop reads and surfaces the EOF
    /// without waiting for the deadline), `POLLNVAL` is an immediate
    /// structured error, and a scripted poll failure surfaces first.
    #[test]
    fn poll_classifies_hup_nval_and_stream_readiness() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::new("mock"));
        let fd = sys.open(&path).unwrap();

        // Exhausted stream: idle.
        assert!(matches!(sys.poll(fd, Duration::ZERO), Ok(false)));

        // Pending read chunk: ready.
        sys.device(&path)
            .unwrap()
            .borrow_mut()
            .push_event(0, 0, 1, 1, 0);
        assert!(matches!(sys.poll(fd, Duration::ZERO), Ok(true)));

        // A hung-up device (unplugged, stream exhausted): ready — the loop
        // reads and surfaces the EOF instead of idling until the deadline.
        sys.device(&path).unwrap().borrow_mut().poll_hup = true;
        assert!(matches!(sys.poll(fd, Duration::ZERO), Ok(true)));

        // POLLNVAL: an immediate structured error, never idle.
        sys.device(&path).unwrap().borrow_mut().poll_hup = false;
        sys.device(&path).unwrap().borrow_mut().poll_nval = true;
        assert!(matches!(
            sys.poll(fd, Duration::ZERO),
            Err(SysError::InvalidArgument(_))
        ));

        // A scripted poll failure surfaces first (e.g. EINTR).
        sys.device(&path).unwrap().borrow_mut().poll_nval = false;
        sys.device(&path).unwrap().borrow_mut().poll_error = Some(MockFailure::Interrupted);
        assert!(matches!(
            sys.poll(fd, Duration::ZERO),
            Err(SysError::Interrupted)
        ));
    }
}
