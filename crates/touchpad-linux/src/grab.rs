//! RAII owner of an open evdev device and its optional `EVIOCGRAB` (M4).
//!
//! [`DeviceHandle`] is the grab guard required by M4:
//!
//! * **Grab is explicit opt-in** — a handle is created ungrabbed by
//!   [`DeviceHandle::open`]; only an explicit [`DeviceHandle::grab`] call
//!   issues `EVIOCGRAB(1)`.
//! * **Release is attempted at most once** (M4 review R5) — the handle
//!   tracks "release attempted" separately from "grab known held", so
//!   [`DeviceHandle::ungrab`] and [`DeviceHandle::close`] may be called any
//!   number of times (normal exit, error path, repeated shutdown) but issue
//!   `EVIOCGRAB(0)`/`close(2)` at most once each — even when the ungrab
//!   ioctl itself fails.
//! * **`Drop` is a best-effort fallback only** — it releases the grab and
//!   closes the fd when the caller did not, ignoring errors; explicit
//!   shutdown must use the fallible methods so failures can be reported.
//!
//! Fail-open property: closing the fd implicitly releases the grab in the
//! kernel, so even a failed `EVIOCGRAB(0)` leaves the device released once
//! the handle is closed — a decoder/resync fatal error can therefore always
//! restore the system's control of the touchpad (M4 requirement 4).

#![forbid(unsafe_code)]

use std::path::Path;
use std::rc::Rc;

use crate::sys::{Fd, Sys, SysError};

/// Failure of a grab/ungrab operation.
#[derive(Debug, thiserror::Error)]
pub enum GrabError {
    /// The `EVIOCGRAB` ioctl itself failed.
    #[error("EVIOCGRAB failed: {0}")]
    Io(SysError),
    /// The operation targeted a handle whose fd was already closed.
    #[error("grab operation on a closed device handle")]
    Closed,
}

/// An open device node with an optional, explicitly requested grab.
///
/// Holds an [`Rc`] clone of the shared [`Sys`] seam, so the same system
/// access is available to the read loop, the grab guard, and the resync
/// snapshot adapter without any lifetime coupling.
pub struct DeviceHandle {
    sys: Rc<dyn Sys>,
    fd: Fd,
    /// Whether `EVIOCGRAB(1)` is currently held (i.e. a grab was issued and
    /// no release has succeeded yet).
    grabbed: bool,
    /// Whether `EVIOCGRAB(0)` was already attempted for the current grab
    /// state. This is tracked **separately** from `grabbed` so a failed
    /// release is never retried — the fd close releases the grab in the
    /// kernel (fail-open) and a second ioctl would be both useless and
    /// contrary to the documented at-most-once contract (M4 review R5).
    release_attempted: bool,
    /// Whether the fd was closed (makes `close` idempotent and gates
    /// operations after shutdown).
    closed: bool,
}

impl DeviceHandle {
    /// Opens `path` read-only through the sys seam. The device is **not**
    /// grabbed; grabbing requires the explicit [`DeviceHandle::grab`].
    pub fn open(sys: Rc<dyn Sys>, path: &Path) -> Result<Self, SysError> {
        let fd = sys.open(path)?;
        Ok(Self {
            sys,
            fd,
            grabbed: false,
            release_attempted: false,
            closed: false,
        })
    }

    /// The open fd, or `None` once the handle is closed.
    #[must_use]
    pub fn fd(&self) -> Option<Fd> {
        if self.closed {
            None
        } else {
            Some(self.fd)
        }
    }

    /// Whether `EVIOCGRAB(1)` is currently held.
    #[must_use]
    pub fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    /// Explicitly grabs the device (`EVIOCGRAB(1)`). Idempotent: grabbing an
    /// already-grabbed handle is a no-op.
    pub fn grab(&mut self) -> Result<(), GrabError> {
        if self.closed {
            return Err(GrabError::Closed);
        }
        if self.grabbed {
            return Ok(());
        }
        self.sys.ioctl_grab(self.fd, true).map_err(GrabError::Io)?;
        self.grabbed = true;
        // A fresh grab needs a fresh release attempt.
        self.release_attempted = false;
        Ok(())
    }

    /// Releases the grab (`EVIOCGRAB(0)`). Idempotent: releasing an
    /// ungrabbed (or already-released) handle is a no-op, releasing a closed
    /// handle is a no-op (the fd close already released it), and a **failed**
    /// release is never retried — the release is attempted at most once
    /// (M4 review R5).
    pub fn ungrab(&mut self) -> Result<(), GrabError> {
        if self.closed || self.release_attempted {
            return Ok(());
        }
        if !self.grabbed {
            // Nothing was ever grabbed: nothing to release, and no later
            // call needs to try either.
            self.release_attempted = true;
            return Ok(());
        }
        self.release_attempted = true;
        match self.sys.ioctl_grab(self.fd, false) {
            Ok(()) => {
                self.grabbed = false;
                Ok(())
            }
            Err(error) => {
                // The kernel still holds the grab (the ioctl failed), so
                // `grabbed` stays true; but the release was already
                // attempted, so `close` will not try again — closing the fd
                // releases the grab in the kernel (fail-open).
                Err(GrabError::Io(error))
            }
        }
    }

    /// Closes the fd. Idempotent: closing an already-closed handle succeeds.
    ///
    /// If the grab is still held the release is attempted first (at most
    /// once, best-effort); even if that `EVIOCGRAB(0)` fails, closing the fd
    /// releases the grab in the kernel (fail-open).
    pub fn close(&mut self) -> Result<(), SysError> {
        if self.closed {
            return Ok(());
        }
        let _ = self.ungrab();
        let result = self.sys.close(self.fd);
        self.closed = true;
        result
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        // Best-effort fallback only: the explicit shutdown path uses the
        // fallible methods so failures can be reported. Errors are ignored
        // here on purpose — Drop cannot fail.
        if self.closed {
            return;
        }
        let _ = self.ungrab();
        let _ = self.sys.close(self.fd);
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::rc::Rc;

    use crate::sys::mock::{MockCall, MockDevice, MockFailure, MockSys};

    fn handle(sys: &Rc<MockSys>, name: &str) -> DeviceHandle {
        let sys_rc: Rc<dyn Sys> = sys.clone();
        let path = PathBuf::from(format!("/dev/input/{name}"));
        sys.add_device(&path, MockDevice::new("mock"));
        DeviceHandle::open(sys_rc, &path).unwrap()
    }

    #[test]
    fn grab_is_explicit_opt_in() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        assert!(!handle.is_grabbed());
        // No EVIOCGRAB happened yet.
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
        handle.grab().unwrap();
        assert!(handle.is_grabbed());
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);
    }

    #[test]
    fn double_grab_is_a_noop() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        handle.grab().unwrap();
        handle.grab().unwrap();
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);
    }

    #[test]
    fn ungrab_is_idempotent() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        handle.grab().unwrap();
        handle.ungrab().unwrap();
        handle.ungrab().unwrap();
        assert!(!handle.is_grabbed());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
    }

    #[test]
    fn ungrab_without_grab_is_a_noop() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        handle.ungrab().unwrap();
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    }

    #[test]
    fn close_is_idempotent_and_releases_first() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        handle.grab().unwrap();
        handle.close().unwrap();
        handle.close().unwrap();
        assert!(handle.fd().is_none());
        // One ungrab then one close, in that order.
        let log = sys.log();
        let ungrab_pos = log
            .iter()
            .position(|call| matches!(call, MockCall::Grab(_, false)))
            .expect("ungrab");
        let close_pos = log
            .iter()
            .position(|call| matches!(call, MockCall::Close(_)))
            .expect("close");
        assert!(ungrab_pos < close_pos, "ungrab must precede close");
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    #[test]
    fn operations_on_a_closed_handle_are_gated() {
        let sys = Rc::new(MockSys::new());
        let mut handle = handle(&sys, "event0");
        handle.grab().unwrap();
        handle.close().unwrap();
        assert!(matches!(handle.grab(), Err(GrabError::Closed)));
        assert_eq!(handle.fd(), None);
        // ungrab after close is a safe no-op.
        assert!(handle.ungrab().is_ok());
    }

    #[test]
    fn drop_releases_grab_and_closes_as_fallback() {
        let sys = Rc::new(MockSys::new());
        {
            let mut handle = handle(&sys, "event0");
            handle.grab().unwrap();
        }
        // Drop must have ungrabed and closed (best-effort fallback).
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    #[test]
    fn drop_without_grab_only_closes() {
        let sys = Rc::new(MockSys::new());
        {
            let _ = handle(&sys, "event0");
        }
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    #[test]
    fn grab_ioctl_failure_is_propagated() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::new("mock");
        device.ioctl_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut handle = DeviceHandle::open(sys.clone(), &path).unwrap();
        assert!(matches!(handle.grab(), Err(GrabError::Io(_))));
        assert!(!handle.is_grabbed());
        // The handle remains usable for close.
        handle.close().unwrap();
    }

    /// M4 review R5: a failed `EVIOCGRAB(0)` is attempted at most once; a
    /// second ungrab and a close must not issue a second release ioctl, and
    /// the fd must still be closed (fail-open).
    #[test]
    fn failed_ungrab_is_attempted_at_most_once() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::new("mock");
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut handle = DeviceHandle::open(sys.clone(), &path).unwrap();
        handle.grab().unwrap();
        assert!(matches!(handle.ungrab(), Err(GrabError::Io(_))));
        assert!(handle.is_grabbed(), "a failed release leaves the grab held");
        // A second ungrab is a no-op: the release was already attempted.
        assert!(handle.ungrab().is_ok());
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        // close() still closes the fd without a second release attempt.
        handle.close().unwrap();
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    /// M4 review R5: closing a grabbed handle whose release fails still
    /// issues exactly one `EVIOCGRAB(0)` and one `close`, in that order.
    #[test]
    fn close_with_failed_ungrab_releases_once_then_closes() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::new("mock");
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let mut handle = DeviceHandle::open(sys.clone(), &path).unwrap();
        handle.grab().unwrap();
        handle.close().unwrap();
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        let log = sys.log();
        let release = log
            .iter()
            .position(|call| matches!(call, MockCall::Grab(_, false)))
            .expect("one release attempt");
        let close = log
            .iter()
            .position(|call| matches!(call, MockCall::Close(_)))
            .expect("one close");
        assert!(release < close, "release must precede close");
    }

    /// M4 review R5: Drop with a failing release still performs exactly one
    /// release attempt and one close (best-effort fallback).
    #[test]
    fn drop_with_failed_ungrab_releases_once_and_closes() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::new("mock");
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        {
            let mut handle = DeviceHandle::open(sys.clone(), &path).unwrap();
            handle.grab().unwrap();
        }
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    /// Re-grabbing after a successful release re-arms the release attempt.
    #[test]
    fn regrab_after_release_rearms_the_release() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::new("mock"));
        let mut handle = DeviceHandle::open(sys.clone(), &path).unwrap();
        handle.grab().unwrap();
        handle.ungrab().unwrap();
        handle.grab().unwrap();
        handle.ungrab().unwrap();
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            2
        );
    }

    #[test]
    fn open_failure_is_propagated() {
        let sys = Rc::new(MockSys::new());
        sys.set_open_error(
            PathBuf::from("/dev/input/event0"),
            MockFailure::PermissionDenied,
        );
        let err = match DeviceHandle::open(sys.clone(), Path::new("/dev/input/event0")) {
            Err(error) => error,
            Ok(_) => panic!("expected open to fail"),
        };
        assert!(matches!(err, SysError::PermissionDenied { .. }));
    }
}
