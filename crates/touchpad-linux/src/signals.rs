//! Controlled `SIGINT`/`SIGTERM` stop for the record pipeline (M5).
//!
//! The record command installs a handler for `SIGINT` and `SIGTERM` that
//! records a stop request in a **process-lifetime static**
//! [`std::sync::atomic::AtomicBool`] (M5 re-review R1). The handler is
//! registered **without** `SA_RESTART`, so a pending signal interrupts the
//! blocking `read(2)` on the device fd; the [`crate::sys`] seam surfaces that
//! as [`crate::sys::SysError::Interrupted`] (the M4 EINTR seam), and the
//! input runtime maps "EINTR + stop requested" to a graceful stop
//! ([`crate::runtime::RuntimeError::Interrupted`]) instead of an ordinary
//! fatal error. The caller also polls [`termination_requested`] (and its own
//! stop flag) between steps, covering signals that arrive while the runtime
//! is *not* blocked in a read.
//!
//! ## Memory safety (M5 re-review R1) — the handler owns no caller memory
//!
//! [`install_termination_handler`] takes **no flag argument**: the async
//! handler's only side effect is a store into the process-lifetime static
//! stop state ([`crate::sys::ffi::termination_requested`] on Linux), so the
//! safe API cannot be made to dereference freed caller storage by any
//! teardown interleaving. There is no caller allocation on the async handler
//! path to reclaim: restoring a disposition does not wait for an already-
//! running handler, but that handler's remaining work is a store into
//! never-reclaimed `'static` memory, which is safe no matter when it resumes.
//! The previous design's race (an in-flight handler dereferencing a freed
//! `Arc<AtomicBool>` after guard teardown) is eliminated by construction.
//!
//! [`TerminationHandlerGuard`] restores the previous dispositions when
//! dropped and resets the stop static. Only **one** handler may be installed
//! at a time: a second [`install_termination_handler`] while a guard is
//! alive returns [`SignalError::AlreadyInstalled`] (structured, not a silent
//! overwrite), and a fresh install succeeds (from a clean stop state) after
//! the first guard is dropped. On non-Linux targets the installation is a
//! no-op — the portable offline-replay and mock paths never block on a
//! device — and [`termination_requested`] is always `false` there.
//!
//! ## Signal-safety and concurrency boundary
//!
//! The handler runs on an arbitrary thread at an arbitrary point of the
//! process. Its entire body is one lock-free atomic store into the
//! process-lifetime static (async-signal-safe: no allocation, no locks, no
//! caller-owned memory). Installation and removal are documented
//! single-threaded (the CLI is a single-threaded process, M4 §7) for
//! deterministic disposition-restoration ordering — a convention, **not** a
//! memory-safety requirement, since the handler target is `'static` and
//! never reclaimed. The single-active-install marker is enforced in code.
//!
//! ## Cleanup guarantees
//!
//! As with every other cleanup in this milestone: a handled `SIGINT`/`SIGTERM`
//! runs the ordered shutdown (stop work → output lifecycle no-op → recorder
//! finish → idempotent ungrab → close). `SIGKILL`, a kernel crash, or a hard
//! power loss cannot run any userspace cleanup (design.md §14,
//! IMPLEMENTATION_BRIEF §6).
//!
//! This module is `unsafe`-free: the only `unsafe` in the signal path lives
//! in the Linux FFI adapter ([`crate::sys::ffi`]), the existing minimal
//! FFI/ioctl boundary.
#![forbid(unsafe_code)]

use std::io;

/// Failure of [`install_termination_handler`].
#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    /// Another termination handler is already installed; only one may be
    /// active at a time (M5 review R1).
    #[error("a SIGINT/SIGTERM termination handler is already installed; only one may be active at a time (drop the first guard before installing another)")]
    AlreadyInstalled,
    /// `sigaction(2)` failed for `SIGINT`/`SIGTERM`.
    #[error("could not install the SIGINT/SIGTERM signal handler: {0}")]
    Install(#[from] io::Error),
}

/// RAII guard restoring the previous `SIGINT`/`SIGTERM` dispositions.
///
/// Returned by [`install_termination_handler`]. While alive, the installed
/// handler records a stop request in the process-lifetime static stop state
/// observed via [`termination_requested`]. Dropping the guard restores the
/// dispositions that were in effect before installation, resets the stop
/// state, and clears the single-install marker (a no-op on non-Linux). The
/// handler dereferences no caller-owned memory (M5 re-review R1), so the
/// guard's teardown can never race an in-flight handler over a freed
/// allocation.
pub struct TerminationHandlerGuard {
    #[cfg(target_os = "linux")]
    _inner: crate::sys::ffi::TerminationHandlerGuard,
}

/// Installs a `SIGINT`/`SIGTERM` handler that records a stop request in the
/// process-lifetime stop state (Linux), or a no-op (non-Linux).
///
/// The handler is installed **without** `SA_RESTART`, so a pending signal
/// interrupts a blocking `read(2)`; the runtime's EINTR handling then stops
/// the record session gracefully instead of treating it as an ordinary fatal
/// error.
///
/// The handler dereferences **no caller-owned memory** (M5 re-review R1):
/// callers observe a stop through [`termination_requested`] (or the
/// runtime/record command's stop handling), not through a caller-supplied
/// flag. Only **one** handler may be installed at a time: a second call
/// while a guard is alive fails with [`SignalError::AlreadyInstalled`];
/// after the first guard is dropped, a fresh install succeeds from a clean
/// stop state. (On non-Linux nothing is installed, so installs are no-ops.)
pub fn install_termination_handler() -> Result<TerminationHandlerGuard, SignalError> {
    #[cfg(target_os = "linux")]
    {
        let _inner = match crate::sys::ffi::install_termination_handler() {
            Ok(inner) => inner,
            Err(crate::sys::ffi::TerminationInstallError::AlreadyInstalled) => {
                return Err(SignalError::AlreadyInstalled);
            }
            Err(crate::sys::ffi::TerminationInstallError::Install(error)) => {
                return Err(SignalError::Install(error));
            }
        };
        Ok(TerminationHandlerGuard { _inner })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(TerminationHandlerGuard {})
    }
}

/// Whether the installed termination handler has recorded a stop request
/// since the last install or guard drop (M5 re-review R1).
///
/// Reads the process-lifetime static stop state; safe to call from any
/// thread at any time, with or without a handler installed. The runtime's
/// EINTR handling and the record command's read loop consult this (together
/// with any attached stop flag) to turn an interrupted read into a graceful
/// stop. On non-Linux no handler is ever installed, so this is always
/// `false`.
#[must_use]
pub fn termination_requested() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::sys::ffi::termination_requested()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Serializes tests that install/fire the termination handler or read the
/// process-lifetime stop state: `sys::ffi` and `signals` tests mutate the
/// SIGINT/SIGTERM dispositions and the stop static, and `runtime` tests
/// observe the stop static through `termination_requested`, so all of them
/// must not run concurrently.
#[cfg(test)]
pub(crate) static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the process-global signal state for the duration of each test
    /// (installing the handler mutates the SIGINT/SIGTERM dispositions and
    /// the stop static, so these tests must not run concurrently with each
    /// other or with runtime tests that observe the stop static).
    fn signal_test_guard() -> std::sync::MutexGuard<'static, ()> {
        SIGNAL_TEST_LOCK.lock().unwrap()
    }

    /// The installed handler must record a stop request when invoked (the
    /// handler itself is exercised directly, without delivering a real
    /// signal; the real-signal delivery path is covered by the Linux FFI
    /// test `real_sigint_records_the_stop_request`).
    #[test]
    fn installed_handler_records_a_stop_request_when_fired() {
        let _lock = signal_test_guard();
        let _guard = install_termination_handler().unwrap();
        assert!(!termination_requested());
        // On non-Linux nothing is installed, so firing is impossible and the
        // stop state stays false — the portable no-op is asserted below.
        #[cfg(target_os = "linux")]
        crate::sys::ffi::fire_termination_handler_for_test();
        #[cfg(target_os = "linux")]
        assert!(termination_requested());
        #[cfg(not(target_os = "linux"))]
        assert!(!termination_requested());
    }

    /// M5 review R1 (public API): a second install while the first guard is
    /// alive is rejected with a structured error, the first guard keeps
    /// working, and a fresh install succeeds (from a clean stop state) after
    /// the first guard is dropped.
    #[cfg(target_os = "linux")]
    #[test]
    fn second_install_is_rejected_and_reinstall_works_after_drop() {
        let _lock = signal_test_guard();
        let guard_a = install_termination_handler().unwrap();
        let err = match install_termination_handler() {
            Err(error) => error,
            Ok(_) => panic!("a second install must be rejected"),
        };
        assert!(matches!(err, SignalError::AlreadyInstalled), "{err:?}");
        crate::sys::ffi::fire_termination_handler_for_test();
        assert!(termination_requested());
        drop(guard_a);
        assert!(
            !termination_requested(),
            "guard teardown must reset the stop state"
        );
        let _guard_b = install_termination_handler().unwrap();
        crate::sys::ffi::fire_termination_handler_for_test();
        assert!(termination_requested());
    }

    /// M5 re-review R1 model test (public API): the handler target is
    /// process-lifetime static storage, so an in-flight handler that resumes
    /// after guard teardown stores into never-reclaimed memory — the
    /// previously-unsafe interleaving is deterministic and safe by
    /// construction. Fire the handler, drop the guard (teardown), then fire
    /// again (modelling the in-flight invocation resuming after teardown).
    #[cfg(target_os = "linux")]
    #[test]
    fn in_flight_handler_resuming_after_teardown_is_safe_by_construction() {
        let _lock = signal_test_guard();
        {
            let _guard = install_termination_handler().unwrap();
            crate::sys::ffi::fire_termination_handler_for_test();
            assert!(termination_requested());
        }
        // Teardown complete; an in-flight handler resuming now stores into
        // the same never-reclaimed static:
        crate::sys::ffi::fire_termination_handler_for_test();
        assert!(termination_requested());
        crate::sys::ffi::reset_termination_requested_for_test();
    }
}
