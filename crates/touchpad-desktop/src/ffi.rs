//! Minimal FFI boundary for the **runtime-loaded** libei sender API (M6).
//!
//! # ABI choice (documented, environment-based)
//!
//! This host ships libei/liboeffis **1.6.0** (`libei.so.1.6.0`,
//! pkg-config `libei-1.0`). The adapter loads the versioned soname
//! `libei.so.1` at **run time** with `libloading` rather than linking at
//! build time, so:
//!
//! * the workspace builds and every automated test runs in environments
//!   without the library (M6 constraint: CI/offline replay operable without
//!   portal, display server, session bus, **system library**, hardware or
//!   root);
//! * a missing library is an honest, structured runtime result
//!   ([`DesktopOutputError::LibraryMissing`]), reported by
//!   `touchpadctl output-probe` and blocking `--emit` in the pre-flight
//!   check — never a build failure and never a silent fallback.
//!
//! The symbol set is the libei 1.x sender API (`ei_new_sender`,
//! `ei_setup_backend_fd`, `ei_dispatch`/`ei_get_event`, seat/device
//! capability binding, `ei_device_get_type`, and the pointer/button/scroll
//! emission functions). Missing symbols (an older/different libei) are
//! reported as [`DesktopOutputError::LibraryMissing`] with the symbol name.
//!
//! # Ownership model (M6 re-review R1, R7)
//!
//! The libei handle types are **non-`Copy` RAII owners**, not freely
//! copyable wrappers: [`EiContext`], [`EiSeat`], [`EiDevice`] and [`EiEvent`]
//! each own exactly one libei reference and release it **exactly once** in
//! their `Drop` (via the matching `ei_*_unref` function). Safe code cannot
//! duplicate a handle (no `Clone`/`Copy`), cannot release it twice (only
//! `Drop` runs, once), and cannot use it after release (the borrow checker
//! rejects use of a moved/dropped value). `Libei` methods take **borrowed**
//! handles (`&EiContext`, `&EiSeat`, …); the unref-counted pointers obtained
//! from an event ([`EiSeatRef`], [`EiDeviceRef`]) are lifetime-bound views of
//! the event, so a borrowed pointer can never outlive the event that owns
//! it.
//!
//! **Every owner pins the loaded library itself (M6 re-review R7).** A
//! handle stores an `Arc` clone of the [`libloading::Library`] that keeps
//! the shared object loaded, so dropping the [`Libei`] loader struct — or
//! any other holder — **cannot unload the library while an owner exists**.
//! The `unref` function pointer embedded in a handle is therefore valid for
//! the handle's whole lifetime by construction: the library cannot be
//! unloaded underneath it. This is a type-level guarantee (the `Arc`), not a
//! "only the current caller behaves" comment: safe crate code may create an
//! owner, drop `Libei`, and drop the owner later, and the unref address is
//! still inside a loaded object.
//!
//! This module is **crate-private** (`pub(crate) mod ffi` in `lib.rs`): the
//! unsafe surface is not part of the public API, so no external safe code
//! can name or construct a libei handle at all.
//!
//! # Safety invariants (the only `unsafe` module in this crate)
//!
//! 1. The function pointers are only valid while the library is loaded, and
//!    **every handle holds its own `Arc` to that library** (see the
//!    ownership model above), so the library stays loaded for as long as any
//!    handle exists. `Libei` holds one more `Arc` for its own lifetime.
//!    Event handles are temporaries inside `wait_event` and are dropped
//!    while the `Libei` is alive.
//! 2. Every raw pointer passed to these functions comes from a libei call
//!    this module made, is never aliased by Rust-owned memory, and obeys
//!    libei's ownership rules: `ei_new_sender` returns an owned context
//!    released by `ei_unref`; `ei_setup_backend_fd` **takes ownership** of
//!    the fd and closes it on teardown (the caller must not close it);
//!    seats/devices from events are kept alive with `ei_seat_ref`/
//!    `ei_device_ref` (returning owned RAII handles) and released with the
//!    matching unref before `ei_unref` on the context.
//! 3. All calls happen on a single thread (the CLI main thread); libei
//!    contexts are not thread-safe.
//! 4. `ei_seat_bind_capabilities` is a C variadic function (sentinel
//!    `NULL`); it is called through a variadic function-pointer type with
//!    the fixed argument sequence `(seat, caps..., NULL)`, matching
//!    libei's `va_arg(ap, enum ei_device_capability)` loop.
//!
//! [`crate::native_transport::NativeTransport`] is the only caller and
//! enforces these invariants.

use std::ffi::{c_char, c_double, c_int, c_uint, c_void};
use std::marker::PhantomData;
use std::sync::Arc;

use libloading::Library;

use crate::error::DesktopOutputError;

/// The versioned soname loaded at run time (libei 1.x, present on this host
/// as 1.6.0).
pub const LIBEI_SONAME: &str = "libei.so.1";

/// `EI_EVENT_CONNECT`.
pub const EI_EVENT_CONNECT: c_int = 1;
/// `EI_EVENT_DISCONNECT`.
pub const EI_EVENT_DISCONNECT: c_int = 2;
/// `EI_EVENT_SEAT_ADDED`.
pub const EI_EVENT_SEAT_ADDED: c_int = 3;
/// `EI_EVENT_SEAT_REMOVED`.
pub const EI_EVENT_SEAT_REMOVED: c_int = 4;
/// `EI_EVENT_DEVICE_ADDED`.
pub const EI_EVENT_DEVICE_ADDED: c_int = 5;
/// `EI_EVENT_DEVICE_REMOVED`.
pub const EI_EVENT_DEVICE_REMOVED: c_int = 6;
/// `EI_EVENT_DEVICE_PAUSED`.
pub const EI_EVENT_DEVICE_PAUSED: c_int = 7;
/// `EI_EVENT_DEVICE_RESUMED`.
pub const EI_EVENT_DEVICE_RESUMED: c_int = 8;

/// `EI_DEVICE_CAP_POINTER` (relative pointer).
pub const EI_DEVICE_CAP_POINTER: c_int = 1 << 0;
/// `EI_DEVICE_CAP_POINTER_ABSOLUTE`.
pub const EI_DEVICE_CAP_POINTER_ABSOLUTE: c_int = 1 << 1;
/// `EI_DEVICE_CAP_KEYBOARD`.
pub const EI_DEVICE_CAP_KEYBOARD: c_int = 1 << 2;
/// `EI_DEVICE_CAP_TOUCH`.
pub const EI_DEVICE_CAP_TOUCH: c_int = 1 << 3;
/// `EI_DEVICE_CAP_SCROLL`.
pub const EI_DEVICE_CAP_SCROLL: c_int = 1 << 4;
/// `EI_DEVICE_CAP_BUTTON`.
pub const EI_DEVICE_CAP_BUTTON: c_int = 1 << 5;

/// `EI_DEVICE_TYPE_VIRTUAL` (`libei.h` 1.6: relative deltas are **logical
/// pixels** on the compositor's screen).
pub const EI_DEVICE_TYPE_VIRTUAL: c_int = 1;
/// `EI_DEVICE_TYPE_PHYSICAL` (`libei.h` 1.6: relative deltas are
/// **millimetres** of the physical device — never claimed as pixels by M6).
pub const EI_DEVICE_TYPE_PHYSICAL: c_int = 2;

/// The libei unref function signature (all `ei_*_unref` functions return the
/// pointer they freed, libei style).
type UnrefFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

/// The loaded libei API (see module-level safety invariants).
pub struct Libei {
    /// Kept alive so the function pointers stay valid. **Every handle also
    /// holds its own `Arc` clone of this library** (M6 re-review R7), so the
    /// shared object cannot be unloaded while any handle exists even if this
    /// loader struct is dropped first.
    _lib: Arc<Library>,
    pub(crate) ei_new_sender: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_configure_name: unsafe extern "C" fn(*mut c_void, *const c_char),
    pub(crate) ei_setup_backend_fd: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    pub(crate) ei_get_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(crate) ei_dispatch: unsafe extern "C" fn(*mut c_void),
    pub(crate) ei_get_event: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_event_get_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(crate) ei_event_get_seat: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_event_get_device: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_event_unref: UnrefFn,
    pub(crate) ei_seat_bind_capabilities: unsafe extern "C" fn(*mut c_void, ...),
    pub(crate) ei_seat_ref: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_seat_unref: UnrefFn,
    pub(crate) ei_device_has_capability: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    pub(crate) ei_device_get_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(crate) ei_device_ref: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) ei_device_unref: UnrefFn,
    pub(crate) ei_device_start_emulating: unsafe extern "C" fn(*mut c_void, c_uint),
    pub(crate) ei_device_pointer_motion: unsafe extern "C" fn(*mut c_void, c_double, c_double),
    pub(crate) ei_device_button_button: unsafe extern "C" fn(*mut c_void, c_uint, bool),
    pub(crate) ei_device_scroll_delta: unsafe extern "C" fn(*mut c_void, c_double, c_double),
    pub(crate) ei_device_scroll_stop: unsafe extern "C" fn(*mut c_void, bool, bool),
    pub(crate) ei_device_frame: unsafe extern "C" fn(*mut c_void, u64),
    pub(crate) ei_disconnect: unsafe extern "C" fn(*mut c_void),
    pub(crate) ei_unref: UnrefFn,
    pub(crate) ei_now: unsafe extern "C" fn(*mut c_void) -> u64,
}

/// RAII owner of one libei **context** reference (`ei_new_sender` result).
///
/// Non-`Copy`, non-`Clone`: the context cannot be duplicated. It is released
/// with `ei_unref` **exactly once**, when the owner is dropped. Not
/// constructible by safe code (this module is crate-private and the only
/// constructor is `unsafe`). Holds its own `Arc` to the loaded library, so
/// the library cannot be unloaded while the owner exists (M6 re-review R7).
pub struct EiContext {
    ptr: *mut c_void,
    unref: UnrefFn,
    lib: Arc<Library>,
}

/// RAII owner of one libei **seat** reference (a `ei_seat_ref` result).
///
/// See [`EiContext`] for the ownership model; dropped → `ei_seat_unref`.
pub struct EiSeat {
    ptr: *mut c_void,
    unref: UnrefFn,
    lib: Arc<Library>,
}

/// RAII owner of one libei **device** reference (a `ei_device_ref` result).
///
/// See [`EiContext`] for the ownership model; dropped → `ei_device_unref`.
pub struct EiDevice {
    ptr: *mut c_void,
    unref: UnrefFn,
    lib: Arc<Library>,
}

/// RAII owner of one libei **event** reference (an `ei_get_event` result).
///
/// See [`EiContext`] for the ownership model; dropped → `ei_event_unref`.
pub struct EiEvent {
    ptr: *mut c_void,
    unref: UnrefFn,
    lib: Arc<Library>,
}

/// A **borrowed**, non-owning view of a libei seat pointer taken from an
/// event (`ei_event_get_seat` does not ref-count). Lifetime-bound to the
/// event, so it cannot outlive the event that owns the pointer.
pub struct EiSeatRef<'a> {
    ptr: *mut c_void,
    _marker: PhantomData<&'a EiEvent>,
}

/// A **borrowed**, non-owning view of a libei device pointer taken from an
/// event. Lifetime-bound to the event (see [`EiSeatRef`]).
pub struct EiDeviceRef<'a> {
    ptr: *mut c_void,
    _marker: PhantomData<&'a EiEvent>,
}

impl EiContext {
    /// Wraps a raw libei context pointer.
    ///
    /// # Safety
    /// `ptr` must be a live libei context returned by `ei_new_sender`, or
    /// NULL; `unref` must be the matching `ei_unref` function pointer of the
    /// loaded library; `lib` must be an `Arc` to that same loaded library
    /// (the owner pins it, so the unref address stays valid for the owner's
    /// whole lifetime — M6 re-review R7).
    pub(crate) unsafe fn from_raw(ptr: *mut c_void, unref: UnrefFn, lib: Arc<Library>) -> Self {
        Self { ptr, unref, lib }
    }

    /// Whether the handle is null.
    pub(crate) fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// The number of live references to the pinned library guard (tests
    /// only): the structural proof that the owner itself keeps the library
    /// loaded after the loader's reference is dropped (M6 re-review R7).
    #[cfg(test)]
    pub(crate) fn lib_strong_count(&self) -> usize {
        Arc::strong_count(&self.lib)
    }
}

impl EiSeat {
    /// Wraps a raw libei seat pointer.
    ///
    /// # Safety
    /// `ptr` must be a live, ref-counted libei seat (or NULL for the
    /// no-seat case); `unref` must be the matching `ei_seat_unref` of the
    /// loaded library; `lib` pins the library (see [`EiContext::from_raw`]).
    pub(crate) unsafe fn from_raw(ptr: *mut c_void, unref: UnrefFn, lib: Arc<Library>) -> Self {
        Self { ptr, unref, lib }
    }

    /// The handle value as an opaque u64 (used for
    /// [`crate::transport::SeatId`]).
    pub(crate) fn as_u64(&self) -> u64 {
        self.ptr as u64
    }
}

impl EiDevice {
    /// Wraps a raw libei device pointer.
    ///
    /// # Safety
    /// `ptr` must be a live, ref-counted libei device (or NULL for the
    /// no-device case); `unref` must be the matching `ei_device_unref` of
    /// the loaded library; `lib` pins the library (see
    /// [`EiContext::from_raw`]).
    pub(crate) unsafe fn from_raw(ptr: *mut c_void, unref: UnrefFn, lib: Arc<Library>) -> Self {
        Self { ptr, unref, lib }
    }

    /// The handle value as an opaque u64 (used for
    /// [`crate::transport::DeviceId`]).
    pub(crate) fn as_u64(&self) -> u64 {
        self.ptr as u64
    }
}

impl EiEvent {
    /// Wraps a raw libei event pointer.
    ///
    /// # Safety
    /// `ptr` must be a live libei event owned by the caller, or NULL; `unref`
    /// must be the matching `ei_event_unref` of the loaded library; `lib`
    /// pins the library (see [`EiContext::from_raw`]).
    pub(crate) unsafe fn from_raw(ptr: *mut c_void, unref: UnrefFn, lib: Arc<Library>) -> Self {
        Self { ptr, unref, lib }
    }

    /// Whether the handle is null.
    pub(crate) fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// The raw libei pointer, for the native-adapter test seam below raw
    /// FFI (M6 re-review R8).
    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.ptr
    }
}

impl<'a> EiSeatRef<'a> {
    /// Wraps a borrowed libei seat pointer (lifetime-bound to the event that
    /// owns it). Test-seam constructor: the scripted FFI fake builds raw
    /// borrowed views below raw FFI (M6 re-review R8).
    ///
    /// # Safety
    /// `ptr` must be a live libei seat pointer owned by the event this view
    /// borrows; the lifetime `'a` ties the view to that event.
    #[cfg(test)]
    pub(crate) unsafe fn from_raw(ptr: *mut c_void) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Whether the borrowed seat pointer is null.
    pub(crate) fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// The pointer value as an opaque u64.
    pub(crate) fn as_u64(&self) -> u64 {
        self.ptr as u64
    }

    /// The raw libei pointer, for the native-adapter test seam below raw
    /// FFI (M6 re-review R8).
    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.ptr
    }
}

impl<'a> EiDeviceRef<'a> {
    /// Wraps a borrowed libei device pointer (lifetime-bound to the event
    /// that owns it). Test-seam constructor: the scripted FFI fake builds
    /// raw borrowed views below raw FFI (M6 re-review R8).
    ///
    /// # Safety
    /// `ptr` must be a live libei device pointer owned by the event this
    /// view borrows; the lifetime `'a` ties the view to that event.
    #[cfg(test)]
    pub(crate) unsafe fn from_raw(ptr: *mut c_void) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Whether the borrowed device pointer is null.
    pub(crate) fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// The pointer value as an opaque u64.
    pub(crate) fn as_u64(&self) -> u64 {
        self.ptr as u64
    }

    /// The raw libei pointer, for the native-adapter test seam below raw
    /// FFI (M6 re-review R8).
    #[cfg(test)]
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.ptr
    }
}

impl Drop for EiContext {
    fn drop(&mut self) {
        // Hold the library pin for the whole drop: the `unref` function
        // pointer must stay inside a loaded object (M6 re-review R7).
        let _pinned = &self.lib;
        if !self.ptr.is_null() {
            // SAFETY: the owner holds its own `Arc` to the loaded library
            // (M6 re-review R7), so the `unref` function pointer is still
            // inside a loaded object; this is the one and only unref for
            // this context reference.
            unsafe {
                (self.unref)(self.ptr);
            }
        }
    }
}

impl Drop for EiSeat {
    fn drop(&mut self) {
        // Hold the library pin for the whole drop: the `unref` function
        // pointer must stay inside a loaded object (M6 re-review R7).
        let _pinned = &self.lib;
        if !self.ptr.is_null() {
            // SAFETY: the owner pins the loaded library (see
            // `EiContext::drop`); balances exactly one `ei_seat_ref`.
            unsafe {
                (self.unref)(self.ptr);
            }
        }
    }
}

impl Drop for EiDevice {
    fn drop(&mut self) {
        // Hold the library pin for the whole drop: the `unref` function
        // pointer must stay inside a loaded object (M6 re-review R7).
        let _pinned = &self.lib;
        if !self.ptr.is_null() {
            // SAFETY: the owner pins the loaded library (see
            // `EiContext::drop`); balances exactly one `ei_device_ref`.
            unsafe {
                (self.unref)(self.ptr);
            }
        }
    }
}

impl Drop for EiEvent {
    fn drop(&mut self) {
        // Hold the library pin for the whole drop: the `unref` function
        // pointer must stay inside a loaded object (M6 re-review R7).
        let _pinned = &self.lib;
        if !self.ptr.is_null() {
            // SAFETY: the owner pins the loaded library (see
            // `EiContext::drop`); `ei_get_event` returned an owned event
            // that must be released exactly once.
            unsafe {
                (self.unref)(self.ptr);
            }
        }
    }
}

/// Loads the libei soname and resolves the sender API symbol set.
///
/// A missing library or a missing symbol is an honest
/// [`DesktopOutputError::LibraryMissing`] — never a panic and never a
/// silent success.
pub fn load_libei() -> Result<Libei, DesktopOutputError> {
    unsafe {
        // SAFETY: `Library::new` only opens the shared object; no code is
        // executed. Each symbol is resolved with `get`, whose returned
        // `Symbol` borrow ends before the `Library` is moved into the
        // result; the copied function pointers are only valid for as long
        // as the `Library` stays loaded (module invariant 1).
        let lib = Library::new(LIBEI_SONAME)
            .map_err(|error| DesktopOutputError::LibraryMissing(error.to_string()))?;

        macro_rules! resolve {
            ($field:ident, $name:literal, $ty:ty) => {
                let $field: $ty =
                    *lib.get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|error| {
                            DesktopOutputError::LibraryMissing(format!(
                                "{}: missing symbol {}: {error}",
                                LIBEI_SONAME, $name
                            ))
                        })?;
            };
        }

        resolve!(
            ei_new_sender,
            "ei_new_sender",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_configure_name,
            "ei_configure_name",
            unsafe extern "C" fn(*mut c_void, *const c_char)
        );
        resolve!(
            ei_setup_backend_fd,
            "ei_setup_backend_fd",
            unsafe extern "C" fn(*mut c_void, c_int) -> c_int
        );
        resolve!(
            ei_get_fd,
            "ei_get_fd",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        resolve!(
            ei_dispatch,
            "ei_dispatch",
            unsafe extern "C" fn(*mut c_void)
        );
        resolve!(
            ei_get_event,
            "ei_get_event",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_event_get_type,
            "ei_event_get_type",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        resolve!(
            ei_event_get_seat,
            "ei_event_get_seat",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_event_get_device,
            "ei_event_get_device",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_event_unref,
            "ei_event_unref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_seat_bind_capabilities,
            "ei_seat_bind_capabilities",
            unsafe extern "C" fn(*mut c_void, ...)
        );
        resolve!(
            ei_seat_ref,
            "ei_seat_ref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_seat_unref,
            "ei_seat_unref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_device_has_capability,
            "ei_device_has_capability",
            unsafe extern "C" fn(*mut c_void, c_int) -> bool
        );
        resolve!(
            ei_device_get_type,
            "ei_device_get_type",
            unsafe extern "C" fn(*mut c_void) -> c_int
        );
        resolve!(
            ei_device_ref,
            "ei_device_ref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_device_unref,
            "ei_device_unref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(
            ei_device_start_emulating,
            "ei_device_start_emulating",
            unsafe extern "C" fn(*mut c_void, c_uint)
        );
        resolve!(
            ei_device_pointer_motion,
            "ei_device_pointer_motion",
            unsafe extern "C" fn(*mut c_void, c_double, c_double)
        );
        resolve!(
            ei_device_button_button,
            "ei_device_button_button",
            unsafe extern "C" fn(*mut c_void, c_uint, bool)
        );
        resolve!(
            ei_device_scroll_delta,
            "ei_device_scroll_delta",
            unsafe extern "C" fn(*mut c_void, c_double, c_double)
        );
        resolve!(
            ei_device_scroll_stop,
            "ei_device_scroll_stop",
            unsafe extern "C" fn(*mut c_void, bool, bool)
        );
        resolve!(
            ei_device_frame,
            "ei_device_frame",
            unsafe extern "C" fn(*mut c_void, u64)
        );
        resolve!(
            ei_disconnect,
            "ei_disconnect",
            unsafe extern "C" fn(*mut c_void)
        );
        resolve!(
            ei_unref,
            "ei_unref",
            unsafe extern "C" fn(*mut c_void) -> *mut c_void
        );
        resolve!(ei_now, "ei_now", unsafe extern "C" fn(*mut c_void) -> u64);

        Ok(Libei {
            _lib: Arc::new(lib),
            ei_new_sender,
            ei_configure_name,
            ei_setup_backend_fd,
            ei_get_fd,
            ei_dispatch,
            ei_get_event,
            ei_event_get_type,
            ei_event_get_seat,
            ei_event_get_device,
            ei_event_unref,
            ei_seat_bind_capabilities,
            ei_seat_ref,
            ei_seat_unref,
            ei_device_has_capability,
            ei_device_get_type,
            ei_device_ref,
            ei_device_unref,
            ei_device_start_emulating,
            ei_device_pointer_motion,
            ei_device_button_button,
            ei_device_scroll_delta,
            ei_device_scroll_stop,
            ei_device_frame,
            ei_disconnect,
            ei_unref,
            ei_now,
        })
    }
}

impl Libei {
    /// Loads libei (see [`load_libei`]).
    pub fn load() -> Result<Libei, DesktopOutputError> {
        load_libei()
    }

    /// `ei_new_sender` — creates a sender context (owned; released exactly
    /// once when the returned [`EiContext`] is dropped). May be null on
    /// failure.
    pub(crate) fn new_sender(&self) -> EiContext {
        // SAFETY: `ei_new_sender` takes no other arguments and returns an
        // owned pointer or NULL (module invariant 2); the returned handle
        // is released by `ei_unref` in its `Drop`, exactly once, and pins
        // the library itself (M6 re-review R7).
        unsafe {
            EiContext::from_raw(
                (self.ei_new_sender)(std::ptr::null_mut()),
                self.ei_unref,
                Arc::clone(&self._lib),
            )
        }
    }

    /// `ei_configure_name` — client name for the authorization dialog.
    pub(crate) fn configure_name(&self, ei: &EiContext, name: &std::ffi::CStr) {
        // SAFETY: `ei` is a live context handle (module invariant 1/2);
        // `name` is a valid NUL-terminated C string.
        unsafe { (self.ei_configure_name)(ptr_of(ei), name.as_ptr()) }
    }

    /// `ei_setup_backend_fd` — takes ownership of `fd` (closes it on
    /// teardown). Returns zero on success or a negative errno.
    pub(crate) fn setup_backend_fd(&self, ei: &EiContext, fd: c_int) -> c_int {
        // SAFETY: `ei` is live; `fd` is a valid open fd whose ownership
        // transfers to libei (the caller must not close it afterwards).
        unsafe { (self.ei_setup_backend_fd)(ptr_of(ei), fd) }
    }

    /// `ei_get_fd` — the pollable fd of a connected context.
    pub(crate) fn get_fd(&self, ei: &EiContext) -> c_int {
        // SAFETY: `ei` is a live, connected context handle.
        unsafe { (self.ei_get_fd)(ptr_of(ei)) }
    }

    /// `ei_dispatch` — processes whatever arrived on the fd and flushes
    /// queued outgoing data.
    pub(crate) fn dispatch(&self, ei: &EiContext) {
        // SAFETY: `ei` is a live, connected context handle.
        unsafe { (self.ei_dispatch)(ptr_of(ei)) }
    }

    /// `ei_get_event` — next queued event (owned; released exactly once
    /// when the returned [`EiEvent`] is dropped), or a null handle.
    pub(crate) fn get_event(&self, ei: &EiContext) -> EiEvent {
        // SAFETY: `ei` is a live context handle.
        unsafe {
            EiEvent::from_raw(
                (self.ei_get_event)(ptr_of(ei)),
                self.ei_event_unref,
                Arc::clone(&self._lib),
            )
        }
    }

    /// `ei_event_get_type` — the event type.
    pub(crate) fn event_get_type(&self, event: &EiEvent) -> c_int {
        // SAFETY: `event` is a live event handle owned by the caller.
        unsafe { (self.ei_event_get_type)(ptr_of(event)) }
    }

    /// `ei_event_get_seat` — the event's seat. **Not** ref-counted by this
    /// call: the returned view is lifetime-bound to `event`.
    pub(crate) fn event_get_seat<'a>(&self, event: &'a EiEvent) -> EiSeatRef<'a> {
        // SAFETY: `event` is a live event handle; the seat pointer is valid
        // for as long as the event is alive (libei: seats/regions owned by
        // the event) — encoded by the returned lifetime.
        unsafe {
            EiSeatRef {
                ptr: (self.ei_event_get_seat)(ptr_of(event)),
                _marker: PhantomData,
            }
        }
    }

    /// `ei_event_get_device` — the event's device. **Not** ref-counted by
    /// this call: the returned view is lifetime-bound to `event`.
    pub(crate) fn event_get_device<'a>(&self, event: &'a EiEvent) -> EiDeviceRef<'a> {
        // SAFETY: `event` is a live event handle; the device pointer is
        // valid for as long as the event is alive — encoded by the returned
        // lifetime.
        unsafe {
            EiDeviceRef {
                ptr: (self.ei_event_get_device)(ptr_of(event)),
                _marker: PhantomData,
            }
        }
    }

    /// `ei_seat_bind_capabilities` — binds the seat to the given
    /// capability values (already including the terminating NULL sentinel).
    pub(crate) fn seat_bind_capabilities(&self, seat: &EiSeat, capabilities: &[c_int; 8]) {
        // SAFETY: the variadic call passes the fixed `(seat, caps...,
        // NULL)` sequence matching libei's va_arg loop (module invariant
        // 4); `seat` is a live, ref-counted seat handle.
        unsafe {
            (self.ei_seat_bind_capabilities)(
                ptr_of(seat),
                capabilities[0],
                capabilities[1],
                capabilities[2],
                capabilities[3],
                capabilities[4],
                capabilities[5],
                capabilities[6],
                capabilities[7],
            );
        }
    }

    /// `ei_seat_ref` — takes an **owned** reference to a seat (released
    /// exactly once when the returned [`EiSeat`] is dropped).
    pub(crate) fn seat_ref(&self, seat: &EiSeatRef<'_>) -> EiSeat {
        // SAFETY: `seat` is a live seat pointer borrowed from a live event;
        // `ei_seat_ref` returns an owned reference balanced by
        // `ei_seat_unref` in the handle's `Drop`.
        unsafe {
            EiSeat::from_raw(
                (self.ei_seat_ref)(seat.ptr),
                self.ei_seat_unref,
                Arc::clone(&self._lib),
            )
        }
    }

    /// `ei_device_has_capability` — whether a device has the capability.
    pub(crate) fn device_has_capability(&self, device: &EiDevice, capability: c_int) -> bool {
        // SAFETY: `device` is a live device handle; the query is read-only.
        unsafe { (self.ei_device_has_capability)(ptr_of(device), capability) }
    }

    /// `ei_device_get_type` — the device type (`EI_DEVICE_TYPE_VIRTUAL` /
    /// `EI_DEVICE_TYPE_PHYSICAL`). Read-only.
    pub(crate) fn device_get_type(&self, device: &EiDevice) -> c_int {
        // SAFETY: `device` is a live device handle; the query is read-only.
        unsafe { (self.ei_device_get_type)(ptr_of(device)) }
    }

    /// `ei_device_ref` — takes an **owned** reference to a device (released
    /// exactly once when the returned [`EiDevice`] is dropped).
    pub(crate) fn device_ref(&self, device: &EiDeviceRef<'_>) -> EiDevice {
        // SAFETY: `device` is a live device pointer borrowed from a live
        // event; `ei_device_ref` returns an owned reference balanced by
        // `ei_device_unref` in the handle's `Drop`.
        unsafe {
            EiDevice::from_raw(
                (self.ei_device_ref)(device.ptr),
                self.ei_device_unref,
                Arc::clone(&self._lib),
            )
        }
    }

    /// `ei_device_start_emulating` — marks the start of an emulation
    /// transaction.
    pub(crate) fn device_start_emulating(&self, device: &EiDevice, sequence: c_uint) {
        // SAFETY: `device` is a live, resumed device handle; `sequence`
        // increases by at least 1 per call.
        unsafe { (self.ei_device_start_emulating)(ptr_of(device), sequence) }
    }

    /// `ei_device_pointer_motion` — relative motion (logical pixels).
    pub(crate) fn device_pointer_motion(&self, device: &EiDevice, x: c_double, y: c_double) {
        // SAFETY: `device` is a live device handle in an emulation
        // transaction; x/y are finite.
        unsafe { (self.ei_device_pointer_motion)(ptr_of(device), x, y) }
    }

    /// `ei_device_button_button` — button press/release (Linux input code).
    pub(crate) fn device_button_button(&self, device: &EiDevice, button: c_uint, is_press: bool) {
        // SAFETY: `device` is a live device handle; `is_press` is a valid
        // C `_Bool` value (0/1).
        unsafe { (self.ei_device_button_button)(ptr_of(device), button, is_press) }
    }

    /// `ei_device_scroll_delta` — pixel-precise scroll delta.
    pub(crate) fn device_scroll_delta(&self, device: &EiDevice, x: c_double, y: c_double) {
        // SAFETY: `device` is a live device handle; x/y are finite.
        unsafe { (self.ei_device_scroll_delta)(ptr_of(device), x, y) }
    }

    /// `ei_device_scroll_stop` — scroll stop for the given axes.
    pub(crate) fn device_scroll_stop(&self, device: &EiDevice, stop_x: bool, stop_y: bool) {
        // SAFETY: `device` is a live device handle; both flags are valid
        // C `_Bool` values.
        unsafe { (self.ei_device_scroll_stop)(ptr_of(device), stop_x, stop_y) }
    }

    /// `ei_device_frame` — closes the current logical frame with a
    /// CLOCK_MONOTONIC µs timestamp.
    pub(crate) fn device_frame(&self, device: &EiDevice, time_us: u64) {
        // SAFETY: `device` is a live device handle.
        unsafe { (self.ei_device_frame)(ptr_of(device), time_us) }
    }

    /// `ei_disconnect` — disconnects the context (idempotent).
    pub(crate) fn disconnect(&self, ei: &EiContext) {
        // SAFETY: `ei` is a live context handle.
        unsafe { (self.ei_disconnect)(ptr_of(ei)) }
    }

    /// `ei_now` — the current CLOCK_MONOTONIC timestamp in microseconds.
    pub(crate) fn now(&self, ei: &EiContext) -> u64 {
        // SAFETY: `ei` is a live context handle.
        unsafe { (self.ei_now)(ptr_of(ei)) }
    }
}

/// Unwraps a borrowed handle to its raw pointer (crate-private; the handles
/// can only be constructed from live libei pointers).
fn ptr_of<T: Handle>(handle: &T) -> *mut c_void {
    handle.as_raw()
}

/// Internal access to a handle's raw pointer.
trait Handle {
    fn as_raw(&self) -> *mut c_void;
}

impl Handle for EiContext {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

impl Handle for EiSeat {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

impl Handle for EiSeatRef<'_> {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

impl Handle for EiDevice {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

impl Handle for EiDeviceRef<'_> {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

impl Handle for EiEvent {
    fn as_raw(&self) -> *mut c_void {
        self.ptr
    }
}

/// Wraps `poll(2)` on the libei fd (Linux-only FFI surface used by the
/// native transport's event loop).
///
/// Returns the `pollfd` with `revents` filled in, or an I/O error (a
/// timeout is reported as `revents == 0`, not as an error; `EINTR` is
/// surfaced as `ErrorKind::Interrupted` so the caller can re-check
/// cancellation).
#[cfg(target_os = "linux")]
pub fn poll_fd(fd: c_int, timeout_ms: c_int) -> Result<libc::pollfd, std::io::Error> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` points to a valid, initialized struct; a single fd;
    // `timeout_ms` is a valid `c_int` timeout.
    let rc = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(pollfd)
}

/// Closes a raw fd (Linux-only).
///
/// Used by the native transport to release the EIS fd when
/// `ei_new_sender` fails **before** the fd ownership was transferred to
/// `ei_setup_backend_fd` (after that call, libei owns and closes the fd).
/// Safe wrapper over `close(2)`.
#[cfg(target_os = "linux")]
pub fn close_fd(fd: c_int) {
    // SAFETY: `close(2)` accepts any integer fd; a failure (including an
    // already-closed fd) is reported via errno, never UB.
    unsafe {
        libc::close(fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    // The unref-call counter is **thread-local** so the ownership tests can
    // run in parallel without racing on a shared static (each test thread
    // counts its own handles' drops).
    thread_local! {
        static FAKE_UNREF_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    fn unref_calls() -> usize {
        FAKE_UNREF_CALLS.with(|calls| calls.get())
    }

    fn reset_unref_calls() {
        FAKE_UNREF_CALLS.with(|calls| calls.set(0));
    }

    /// A stand-in for a libei unref function: records the call on the
    /// calling thread and returns the pointer, exactly like the real
    /// `ei_*_unref` functions. Used to prove the RAII ownership model
    /// without loading libei.
    unsafe extern "C" fn fake_unref(ptr: *mut c_void) -> *mut c_void {
        FAKE_UNREF_CALLS.with(|calls| calls.set(calls.get() + 1));
        ptr
    }

    /// A library guard for tests: the host-process library, which is always
    /// loaded and can never be unloaded. Exercises the exact `Arc`-pinning
    /// structure the real `Libei` uses without loading libei (M6 re-review
    /// R7).
    fn test_guard() -> Arc<Library> {
        #[cfg(unix)]
        {
            Arc::new(libloading::os::unix::Library::this().into())
        }
        #[cfg(windows)]
        {
            Arc::new(
                libloading::os::windows::Library::this()
                    .expect("the host-process library is always loadable")
                    .into(),
            )
        }
    }

    /// Loading is a side-effect-free dlopen probe: it succeeds when the
    /// library is present and fails with a structured `LibraryMissing`
    /// otherwise — never a panic. No context is created and nothing is
    /// connected in this test.
    #[test]
    fn load_is_a_structured_probe() {
        match load_libei() {
            Ok(_libei) => {}
            Err(DesktopOutputError::LibraryMissing(message)) => {
                assert!(!message.is_empty());
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    /// The capability bit constants match `libei.h` 1.6.
    #[test]
    fn capability_bits_match_libei_1_6() {
        assert_eq!(EI_DEVICE_CAP_POINTER, 1 << 0);
        assert_eq!(EI_DEVICE_CAP_POINTER_ABSOLUTE, 1 << 1);
        assert_eq!(EI_DEVICE_CAP_KEYBOARD, 1 << 2);
        assert_eq!(EI_DEVICE_CAP_TOUCH, 1 << 3);
        assert_eq!(EI_DEVICE_CAP_SCROLL, 1 << 4);
        assert_eq!(EI_DEVICE_CAP_BUTTON, 1 << 5);
    }

    /// The event constants match `libei.h` 1.6.
    #[test]
    fn event_constants_match_libei_1_6() {
        assert_eq!(EI_EVENT_CONNECT, 1);
        assert_eq!(EI_EVENT_DISCONNECT, 2);
        assert_eq!(EI_EVENT_SEAT_ADDED, 3);
        assert_eq!(EI_EVENT_SEAT_REMOVED, 4);
        assert_eq!(EI_EVENT_DEVICE_ADDED, 5);
        assert_eq!(EI_EVENT_DEVICE_REMOVED, 6);
        assert_eq!(EI_EVENT_DEVICE_PAUSED, 7);
        assert_eq!(EI_EVENT_DEVICE_RESUMED, 8);
    }

    /// The device-type constants match `libei.h` 1.6 (`VIRTUAL = 1`,
    /// `PHYSICAL = 2`; relative deltas are logical pixels only for virtual
    /// devices, millimetres for physical ones).
    #[test]
    fn device_type_constants_match_libei_1_6() {
        assert_eq!(EI_DEVICE_TYPE_VIRTUAL, 1);
        assert_eq!(EI_DEVICE_TYPE_PHYSICAL, 2);
    }

    /// M6 re-review R1: dropping an RAII owner calls the unref function
    /// **exactly once** (one-time destruction; the handles are not `Copy`,
    /// so they cannot be duplicated or released twice).
    #[test]
    fn owner_drop_calls_unref_exactly_once() {
        reset_unref_calls();
        {
            let guard = test_guard();
            let _context = unsafe {
                EiContext::from_raw(
                    std::ptr::dangling_mut::<c_void>(),
                    fake_unref,
                    guard.clone(),
                )
            };
            let _seat = unsafe {
                EiSeat::from_raw(
                    std::ptr::dangling_mut::<c_void>(),
                    fake_unref,
                    guard.clone(),
                )
            };
            let _device = unsafe {
                EiDevice::from_raw(
                    std::ptr::dangling_mut::<c_void>(),
                    fake_unref,
                    guard.clone(),
                )
            };
            let _event = unsafe {
                EiEvent::from_raw(
                    std::ptr::dangling_mut::<c_void>(),
                    fake_unref,
                    guard.clone(),
                )
            };
        }
        assert_eq!(unref_calls(), 4);
    }

    /// M6 re-review R1: a null handle is a valid "no object" value and its
    /// drop is a no-op (libei functions accept NULL; the null handle must
    /// not be unref'd).
    #[test]
    fn null_owner_drop_is_a_no_op() {
        reset_unref_calls();
        {
            let guard = test_guard();
            let _context =
                unsafe { EiContext::from_raw(std::ptr::null_mut(), fake_unref, guard.clone()) };
            let _event =
                unsafe { EiEvent::from_raw(std::ptr::null_mut(), fake_unref, guard.clone()) };
        }
        assert_eq!(unref_calls(), 0);
    }

    /// M6 re-review R1: the handles are not `Clone`/`Copy` — moving an
    /// owner into a consuming function transfers ownership, and the original
    /// binding is dead (the compiler enforces this; the test proves the
    /// value can be moved and that the owner still releases exactly once).
    #[test]
    fn owners_move_but_do_not_copy() {
        reset_unref_calls();
        let seat = unsafe {
            EiSeat::from_raw(std::ptr::dangling_mut::<c_void>(), fake_unref, test_guard())
        };
        // A move is allowed (non-Copy): the original binding is consumed.
        take_seat(seat);
        assert_eq!(unref_calls(), 1);
    }

    fn take_seat(_seat: EiSeat) {}

    /// M6 re-review R7: an owner **pins the loaded library itself** — the
    /// loader's reference to the guard can be dropped while the owner is
    /// alive, and the owner alone keeps the library loaded (observable via
    /// the guard's `Arc` strong count) until its own `Drop` releases the
    /// reference. This is the structural proof that dropping `Libei` cannot
    /// unload the library underneath a live handle, so the embedded `unref`
    /// function pointer stays valid for the owner's whole lifetime.
    #[test]
    fn owner_pins_the_library_after_the_loader_is_dropped() {
        reset_unref_calls();
        let guard = test_guard();
        // The `Libei` loader holds one reference to the guard; the handle
        // will hold another (this is the type-level pin, M6 re-review R7).
        let loader_ref = guard.clone();
        let owner =
            unsafe { EiContext::from_raw(std::ptr::dangling_mut::<c_void>(), fake_unref, guard) };
        assert_eq!(
            owner.lib_strong_count(),
            2,
            "loader + owner both pin the library"
        );

        // Drop the loader reference while the owner is still alive: the
        // library must stay loaded (the owner's Arc is what keeps it).
        drop(loader_ref);
        assert_eq!(
            owner.lib_strong_count(),
            1,
            "the owner alone must keep the library loaded after the loader is dropped"
        );

        // The owner still releases exactly once, using a function pointer
        // that is still inside the (pinned) library.
        drop(owner);
        assert_eq!(unref_calls(), 1);
    }

    /// M6 re-review R7: the borrowed views expose the raw pointer used for
    /// opaque id encoding. The **lifetime** tie to the owning event is
    /// enforced by the `Libei::event_get_seat`/`event_get_device` signatures
    /// (`-> EiSeatRef<'a>` borrowing `&'a EiEvent`), which the native
    /// transport is the only caller of.
    #[test]
    fn borrowed_views_expose_their_pointer() {
        let seat: EiSeatRef<'_> = unsafe { EiSeatRef::from_raw(std::ptr::dangling_mut()) };
        let device: EiDeviceRef<'_> = unsafe { EiDeviceRef::from_raw(std::ptr::dangling_mut()) };
        assert!(!seat.is_null());
        assert!(!device.is_null());
        assert_eq!(seat.as_ptr(), device.as_ptr());
        assert_eq!(seat.as_u64(), device.as_u64());
    }
}
