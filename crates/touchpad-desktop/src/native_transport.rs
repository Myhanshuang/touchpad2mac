#![deny(unsafe_code)]
//! The real libei **sender** transport (M6, Linux-only).
//!
//! [`NativeTransport`] implements [`Transport`] over the runtime-loaded
//! libei FFI ([`crate::ffi`]): it owns the `ei` context and its ref-counted
//! seats/devices (as non-`Copy` RAII owners that release each reference
//! exactly once and pin the loaded library themselves), drives the fd event
//! loop with `poll`, and emits relative pointer motion, buttons, and
//! pixel-precise scroll events inside frames.
//!
//! # Safety
//!
//! This module is the **only caller** of the libei FFI wrappers and enforces
//! their invariants: libei objects are handled exclusively through the
//! crate-private RAII handles of [`crate::ffi`] (which safe code cannot
//! fabricate, duplicate, or double-release), seats and devices are `ref`-ed
//! on add and released exactly once (via `Drop`) in dependency order —
//! devices → seats → context — before the context is unref-ed, the fd
//! ownership transfers to `ei_setup_backend_fd`, and everything happens on
//! one thread. This module contains **no `unsafe` blocks** — the `unsafe`
//! calls are confined to the minimal FFI boundary ([`crate::ffi`]); the
//! module is `#![deny(unsafe_code)]`, and only its scripted test seam
//! (which builds fake raw handles below raw FFI) opts back in with an
//! `#![allow(unsafe_code)]`.
//!
//! # Event-queue discipline (M6 re-review R8)
//!
//! libei queues events internally; they enter that queue **only** when the
//! context is dispatched (`ei_dispatch` reads the fd), and `ei_get_event`
//! pops them one at a time. `wait_event` therefore:
//!
//! 1. delivers already-fetched events from the transport's **native pending
//!    queue** first (a previous dispatch may have queued several events, and
//!    the kernel fd is not readable again until libei's internal queue is
//!    drained);
//! 2. drains libei's internal queue with `ei_get_event` **before** polling
//!    the fd, so events queued by an earlier dispatch are surfaced even when
//!    the fd reports no readiness;
//! 3. only then polls; after a dispatch it drains the **entire** internal
//!    queue into the native pending queue, so two or more events produced by
//!    one `ei_dispatch` are all surfaced even when the following poll
//!    reports no readiness.
//!
//! # Deferred mapping at delivery order (M6 re-review R11)
//!
//! The native pending queue holds the **raw owned [`EiEvent`]s**, not mapped
//! [`TransportEvent`]s. Mapping an event and applying its
//! seat/device/resumed side effects happens **exactly when the event is
//! popped for delivery** ([`Self::map_event`] is only ever called from the
//! delivery path), so a single dispatch containing several lifecycle events
//! applies the state of each event at the point the caller observes it. A
//! later queued `DEVICE_PAUSED`/`DEVICE_REMOVED`/`SEAT_REMOVED`/`DISCONNECT`
//! therefore cannot mutate the transport state before the earlier
//! `DEVICE_RESUMED`/`SEAT_ADDED` events have been delivered: a seat stays
//! bindable until its `SeatRemoved` is actually delivered, a resumed device
//! stays usable until its queued pause/removal is actually delivered, and a
//! queued disconnect becomes terminal only at delivery — without losing the
//! events queued before it. Each raw event owner pins the loaded library
//! itself (M6 re-review R7) and is released exactly once, at delivery or at
//! teardown.
//!
//! # Flushing and write-side errors (M6 re-review R3/R8)
//!
//! libei queues outgoing messages; they are flushed when the context is
//! **dispatched**. [`Transport::pump`] therefore calls `ei_dispatch`
//! **unconditionally** (once per pump), even when the fd reports no
//! readiness — libei's backend fd is nonblocking, so a dispatch with no
//! incoming data returns immediately after flushing — and
//! [`Transport::wait_event`] also dispatches whenever the fd is readable.
//! A write-side failure is reported asynchronously by libei as an
//! `EI_EVENT_DISCONNECT` (the context is put into an error state), which the
//! transport surfaces as the terminal [`TransportEvent::Disconnected`] —
//! that is how the `void` emission wrappers' errors reach the adapter.
//!
//! # Test seam below raw FFI (M6 re-review R8)
//!
//! `NativeTransport` is generic over the [`NativeFfi`] seam — the exact
//! libei surface it uses — so the queue discipline above is testable with a
//! scripted fake ([`tests::ScriptedFfi`]) that mimics libei's real queue
//! contract (events enter the internal queue only via `dispatch`) and a
//! scripted `poll` readiness, **without a real library, a real fd, or any
//! emission**. The real `Libei` implements [`NativeFfi`]; no test constructs
//! a real libei context or emits real desktop input.

use std::collections::VecDeque;
use std::time::Duration;

use crate::error::DesktopOutputError;
use crate::ffi::{
    self, EiContext, EiDevice, EiDeviceRef, EiEvent, EiSeat, EiSeatRef, Libei,
    EI_DEVICE_CAP_BUTTON, EI_DEVICE_CAP_POINTER, EI_DEVICE_CAP_POINTER_ABSOLUTE,
    EI_DEVICE_CAP_SCROLL, EI_EVENT_CONNECT, EI_EVENT_DEVICE_ADDED, EI_EVENT_DEVICE_PAUSED,
    EI_EVENT_DEVICE_REMOVED, EI_EVENT_DEVICE_RESUMED, EI_EVENT_DISCONNECT, EI_EVENT_SEAT_ADDED,
    EI_EVENT_SEAT_REMOVED,
};
use crate::transport::{DeviceId, DeviceType, SeatId, Transport, TransportEvent};

/// The seam between [`NativeTransport`] and the runtime-loaded libei FFI.
///
/// The concrete [`Libei`] implements this trait by delegating to the raw
/// function pointers; the scripted fake in the tests implements the same
/// surface with libei's actual queue contract, so the native event-loop
/// algorithm is testable **below raw FFI** without a real library, fd, or
/// emission (M6 re-review R8).
pub(crate) trait NativeFfi {
    /// `ei_new_sender` — owned context or NULL.
    fn new_sender(&self) -> EiContext;
    /// `ei_configure_name`.
    fn configure_name(&self, ei: &EiContext, name: &std::ffi::CStr);
    /// `ei_setup_backend_fd` — takes ownership of `fd`; 0 on success.
    fn setup_backend_fd(&self, ei: &EiContext, fd: i32) -> i32;
    /// `ei_get_fd` — the pollable fd.
    fn get_fd(&self, ei: &EiContext) -> i32;
    /// `ei_dispatch` — reads the fd into libei's internal queue and flushes
    /// queued outgoing data.
    fn dispatch(&self, ei: &EiContext);
    /// `ei_get_event` — pops the next queued event (owned) or a null handle.
    fn get_event(&self, ei: &EiContext) -> EiEvent;
    /// `ei_event_get_type`.
    fn event_get_type(&self, event: &EiEvent) -> i32;
    /// `ei_event_get_seat` — borrowed view, lifetime-bound to `event`.
    fn event_get_seat<'a>(&self, event: &'a EiEvent) -> EiSeatRef<'a>;
    /// `ei_seat_ref` — owned seat reference.
    fn seat_ref(&self, seat: &EiSeatRef<'_>) -> EiSeat;
    /// `ei_seat_bind_capabilities` — sentinel-terminated capability list.
    fn seat_bind_capabilities(&self, seat: &EiSeat, capabilities: &[i32; 8]);
    /// `ei_event_get_device` — borrowed view, lifetime-bound to `event`.
    fn event_get_device<'a>(&self, event: &'a EiEvent) -> EiDeviceRef<'a>;
    /// `ei_device_ref` — owned device reference.
    fn device_ref(&self, device: &EiDeviceRef<'_>) -> EiDevice;
    /// `ei_device_has_capability`.
    fn device_has_capability(&self, device: &EiDevice, capability: i32) -> bool;
    /// `ei_device_get_type`.
    fn device_get_type(&self, device: &EiDevice) -> i32;
    /// `ei_device_start_emulating`.
    fn device_start_emulating(&self, device: &EiDevice, sequence: u32);
    /// `ei_device_pointer_motion`.
    fn device_pointer_motion(&self, device: &EiDevice, dx: f64, dy: f64);
    /// `ei_device_button_button`.
    fn device_button_button(&self, device: &EiDevice, button: u32, is_press: bool);
    /// `ei_device_scroll_delta`.
    fn device_scroll_delta(&self, device: &EiDevice, dx: f64, dy: f64);
    /// `ei_device_scroll_stop`.
    fn device_scroll_stop(&self, device: &EiDevice, stop_x: bool, stop_y: bool);
    /// `ei_device_frame`.
    fn device_frame(&self, device: &EiDevice, time_us: u64);
    /// `ei_now` — monotonic µs clock.
    fn now(&self, ei: &EiContext) -> u64;
    /// `ei_disconnect`.
    fn disconnect(&self, ei: &EiContext);
    /// `poll(2)` readiness of the transport fd: whether `POLLIN` (or an
    /// error/hangup state) is reported within the timeout.
    fn poll(&self, fd: i32, timeout_ms: i32) -> Result<bool, std::io::Error>;
    /// `close(2)` of an fd that was never transferred to libei.
    fn close_fd(&self, fd: i32);
}

impl NativeFfi for Libei {
    fn new_sender(&self) -> EiContext {
        Libei::new_sender(self)
    }
    fn configure_name(&self, ei: &EiContext, name: &std::ffi::CStr) {
        Libei::configure_name(self, ei, name);
    }
    fn setup_backend_fd(&self, ei: &EiContext, fd: i32) -> i32 {
        Libei::setup_backend_fd(self, ei, fd)
    }
    fn get_fd(&self, ei: &EiContext) -> i32 {
        Libei::get_fd(self, ei)
    }
    fn dispatch(&self, ei: &EiContext) {
        Libei::dispatch(self, ei);
    }
    fn get_event(&self, ei: &EiContext) -> EiEvent {
        Libei::get_event(self, ei)
    }
    fn event_get_type(&self, event: &EiEvent) -> i32 {
        Libei::event_get_type(self, event)
    }
    fn event_get_seat<'a>(&self, event: &'a EiEvent) -> EiSeatRef<'a> {
        Libei::event_get_seat(self, event)
    }
    fn seat_ref(&self, seat: &EiSeatRef<'_>) -> EiSeat {
        Libei::seat_ref(self, seat)
    }
    fn seat_bind_capabilities(&self, seat: &EiSeat, capabilities: &[i32; 8]) {
        Libei::seat_bind_capabilities(self, seat, capabilities);
    }
    fn event_get_device<'a>(&self, event: &'a EiEvent) -> EiDeviceRef<'a> {
        Libei::event_get_device(self, event)
    }
    fn device_ref(&self, device: &EiDeviceRef<'_>) -> EiDevice {
        Libei::device_ref(self, device)
    }
    fn device_has_capability(&self, device: &EiDevice, capability: i32) -> bool {
        Libei::device_has_capability(self, device, capability)
    }
    fn device_get_type(&self, device: &EiDevice) -> i32 {
        Libei::device_get_type(self, device)
    }
    fn device_start_emulating(&self, device: &EiDevice, sequence: u32) {
        Libei::device_start_emulating(self, device, sequence);
    }
    fn device_pointer_motion(&self, device: &EiDevice, dx: f64, dy: f64) {
        Libei::device_pointer_motion(self, device, dx, dy);
    }
    fn device_button_button(&self, device: &EiDevice, button: u32, is_press: bool) {
        Libei::device_button_button(self, device, button, is_press);
    }
    fn device_scroll_delta(&self, device: &EiDevice, dx: f64, dy: f64) {
        Libei::device_scroll_delta(self, device, dx, dy);
    }
    fn device_scroll_stop(&self, device: &EiDevice, stop_x: bool, stop_y: bool) {
        Libei::device_scroll_stop(self, device, stop_x, stop_y);
    }
    fn device_frame(&self, device: &EiDevice, time_us: u64) {
        Libei::device_frame(self, device, time_us);
    }
    fn now(&self, ei: &EiContext) -> u64 {
        Libei::now(self, ei)
    }
    fn disconnect(&self, ei: &EiContext) {
        Libei::disconnect(self, ei);
    }
    fn poll(&self, fd: i32, timeout_ms: i32) -> Result<bool, std::io::Error> {
        let pollfd = crate::ffi::poll_fd(fd, timeout_ms)?;
        Ok(pollfd.revents != 0)
    }
    fn close_fd(&self, fd: i32) {
        crate::ffi::close_fd(fd);
    }
}

/// The real libei sender transport. See the module documentation.
pub struct NativeTransport<F: NativeFfi> {
    /// The libei surface (the loaded library on the real transport, the
    /// scripted fake in tests).
    ffi: F,
    /// The libei context (`ei_new_sender`), `None` when not connected.
    ei: Option<EiContext>,
    /// Ref-counted seats (`ei_seat_ref`-ed on add; released exactly once by
    /// the RAII owner on removal/teardown).
    seats: Vec<EiSeat>,
    /// Ref-counted devices (`ei_device_ref`-ed on add; released exactly once
    /// by the RAII owner on removal/teardown).
    devices: Vec<EiDevice>,
    /// The devices currently **resumed** (a device must be resumed to emit
    /// through it; libei discards events sent on a paused device). Holds the
    /// same RAII owners as `devices` (a device that is resumed is one we
    /// hold a reference to).
    resumed: Vec<EiDevice>,
    /// Raw libei events already fetched from libei's internal queue by a
    /// previous dispatch but not yet delivered to the adapter (M6 re-review
    /// R8). The events are **not** mapped at fetch time: mapping and its
    /// seat/device/resumed side effects run exactly when each event is
    /// popped for delivery (M6 re-review R11), so the transport state the
    /// caller observes is always consistent with the events it has actually
    /// received. Each owner pins the loaded library itself (M6 re-review
    /// R7) and is released exactly once, on delivery or on teardown.
    pending: VecDeque<EiEvent>,
    /// The `ei_device_start_emulating` sequence (must increase by ≥1 per
    /// call).
    next_sequence: u32,
    /// Set once `EI_EVENT_DISCONNECT` was **delivered** (or `disconnect`
    /// ran). Delivered events stay ordered: events fetched before a
    /// disconnect are returned before the terminal `Disconnected`.
    disconnected: bool,
}

impl<F: NativeFfi> NativeTransport<F> {
    /// Creates an unconnected transport over the given libei surface.
    #[must_use]
    pub fn new(ffi: F) -> Self {
        Self {
            ffi,
            ei: None,
            seats: Vec::new(),
            devices: Vec::new(),
            resumed: Vec::new(),
            pending: VecDeque::new(),
            next_sequence: 0,
            disconnected: false,
        }
    }

    fn require_device(&self, device: DeviceId) -> Result<&EiDevice, DesktopOutputError> {
        self.resumed
            .iter()
            .find(|held| held.as_u64() == device.0)
            .ok_or_else(|| {
                DesktopOutputError::Internal(format!(
                    "emitting through device {} which is not currently resumed",
                    device.0
                ))
            })
    }

    /// Maps one raw libei event to a [`TransportEvent`], applying the
    /// seat/device bookkeeping side effects (owned `ref`-counted handles are
    /// taken on add, released on removal). Returns `None` for unmapped
    /// (unknown/irrelevant) event types. Only ever called from the
    /// **delivery path** ([`Self::next_pending_mapped`] pops the raw event
    /// right before mapping it), so the side effects are applied exactly
    /// when the caller observes the corresponding event — never while later
    /// events of the same dispatch are still queued (M6 re-review R11).
    /// `EI_EVENT_DISCONNECT` becomes the terminal `Disconnected` event; the
    /// `disconnected` flag is set when the event is **delivered**
    /// ([`Self::deliver`]), not when it is fetched, so events fetched before
    /// it are still returned first.
    fn map_event(&mut self, event: &EiEvent) -> Option<TransportEvent> {
        let event_type = self.ffi.event_get_type(event);
        match event_type {
            EI_EVENT_CONNECT => Some(TransportEvent::Connected),
            EI_EVENT_DISCONNECT => Some(TransportEvent::Disconnected),
            EI_EVENT_SEAT_ADDED => {
                let seat = self.ffi.event_get_seat(event);
                if seat.is_null() {
                    None
                } else {
                    // Take an owned reference (released exactly once by the
                    // RAII owner when the seat is removed or the transport
                    // tears down).
                    let owned = self.ffi.seat_ref(&seat);
                    self.seats.push(owned);
                    Some(TransportEvent::SeatAdded {
                        seat: SeatId(seat.as_u64()),
                    })
                }
            }
            EI_EVENT_SEAT_REMOVED => {
                let seat = self.ffi.event_get_seat(event);
                if let Some(index) = self
                    .seats
                    .iter()
                    .position(|held| held.as_u64() == seat.as_u64())
                {
                    // swap_remove drops the RAII owner -> seat_unref,
                    // balancing the ref taken on add.
                    self.seats.swap_remove(index);
                }
                Some(TransportEvent::SeatRemoved {
                    seat: SeatId(seat.as_u64()),
                })
            }
            EI_EVENT_DEVICE_ADDED => {
                let device = self.ffi.event_get_device(event);
                if device.is_null() {
                    None
                } else {
                    // Take an owned reference (released exactly once by the
                    // RAII owner on removal/teardown).
                    let owned = self.ffi.device_ref(&device);
                    let capabilities = self.device_capability_bits(&owned);
                    let device_type = DeviceType::from_raw(self.ffi.device_get_type(&owned));
                    self.devices.push(owned);
                    Some(TransportEvent::DeviceAdded {
                        device: DeviceId(device.as_u64()),
                        capabilities,
                        device_type,
                    })
                }
            }
            EI_EVENT_DEVICE_REMOVED => {
                let device = self.ffi.event_get_device(event);
                if let Some(index) = self
                    .devices
                    .iter()
                    .position(|held| held.as_u64() == device.as_u64())
                {
                    // swap_remove drops the RAII owner -> device_unref,
                    // balancing the ref taken on add.
                    self.devices.swap_remove(index);
                }
                self.resumed.retain(|held| held.as_u64() != device.as_u64());
                Some(TransportEvent::DeviceRemoved {
                    device: DeviceId(device.as_u64()),
                })
            }
            EI_EVENT_DEVICE_PAUSED => {
                let device = self.ffi.event_get_device(event);
                // A paused device must not be emitted through until it
                // resumes again.
                self.resumed.retain(|held| held.as_u64() != device.as_u64());
                Some(TransportEvent::DevicePaused {
                    device: DeviceId(device.as_u64()),
                })
            }
            EI_EVENT_DEVICE_RESUMED => {
                let device = self.ffi.event_get_device(event);
                // A resumed device may be emitted through; keep it in the
                // resumed set so `require_device` accepts it (only devices
                // we actually hold are resumed).
                if self
                    .devices
                    .iter()
                    .any(|held| held.as_u64() == device.as_u64())
                    && !self
                        .resumed
                        .iter()
                        .any(|held| held.as_u64() == device.as_u64())
                {
                    let owned = self.ffi.device_ref(&device);
                    self.resumed.push(owned);
                }
                Some(TransportEvent::DeviceResumed {
                    device: DeviceId(device.as_u64()),
                })
            }
            _ => None,
        }
    }

    /// Records the delivery of a mapped event: the terminal `Disconnected`
    /// flips the `disconnected` flag exactly when it is delivered (after any
    /// events fetched before it), keeping delivery order intact.
    fn deliver(&mut self, event: TransportEvent) -> TransportEvent {
        if matches!(event, TransportEvent::Disconnected) {
            self.disconnected = true;
        }
        event
    }

    /// Drains **all** events currently queued inside libei's internal queue
    /// into the native pending queue as **raw owned events** — without
    /// mapping them and without touching the fd (mapping and its
    /// seat/device/resumed side effects happen at delivery, M6 re-review
    /// R11). Called before polling and after every dispatch, so no event can
    /// be stranded behind a fd that stopped reporting readiness (M6
    /// re-review R8).
    fn drain_internal_queue(&mut self) {
        if self.ei.is_none() {
            return;
        }
        loop {
            let event = {
                let ei = self.ei.as_ref().expect("checked above");
                self.ffi.get_event(ei)
            };
            if event.is_null() {
                return;
            }
            self.pending.push_back(event);
            // The event stays owned in `pending`: it is released exactly
            // once, when it is popped for delivery (or at teardown).
        }
    }

    /// Pops the next raw event from the native pending queue and maps it
    /// **at delivery time**, applying the seat/device/resumed side effects
    /// exactly then (M6 re-review R11): an event's state transition becomes
    /// visible to the caller only when the event itself is returned, never
    /// earlier. Events that map to `None` (unknown/null variants) are
    /// dropped without side effects and the loop continues to the next
    /// queued event.
    fn next_pending_mapped(&mut self) -> Option<TransportEvent> {
        while let Some(event) = self.pending.pop_front() {
            if let Some(mapped) = self.map_event(&event) {
                return Some(mapped);
            }
            // The raw event owner drops here — released exactly once; it
            // carried no observable transition.
        }
        None
    }

    /// Releases every held libei reference in dependency order (devices →
    /// seats → context), exactly once per reference (each owner's `Drop`).
    ///
    /// The order matters: libei **devices live inside their seat** (the
    /// protocol guarantees a `DEVICE_REMOVED` event before the matching
    /// `SEAT_REMOVED`), so the refs are released devices → seats → context.
    /// Releasing a seat first could leave a still-referenced device pointing
    /// at a freed seat when its own `device_unref` runs. The `F` (the loaded
    /// library) is deliberately NOT touched here: it outlives every owner.
    fn teardown(&mut self) {
        self.resumed.clear();
        for device in self.devices.drain(..) {
            drop(device);
        }
        for seat in self.seats.drain(..) {
            drop(seat);
        }
        self.pending.clear();
        self.ei = None;
    }
}

impl<F: NativeFfi> Drop for NativeTransport<F> {
    /// Best-effort teardown: an un-disconnected transport still owns a live
    /// libei context and its refs. `teardown` runs before the `F` field
    /// drops; every RAII owner also pins the library itself (M6 re-review
    /// R7), so the unref function pointers stay valid regardless of drop
    /// order.
    fn drop(&mut self) {
        if let Some(ei) = &self.ei {
            self.ffi.disconnect(ei);
        }
        self.teardown();
    }
}

impl<F: NativeFfi> Transport for NativeTransport<F> {
    fn connect(&mut self, fd: i32) -> Result<(), DesktopOutputError> {
        if self.ei.is_some() {
            return Err(DesktopOutputError::Internal(
                "transport already connected".to_string(),
            ));
        }
        let ei = self.ffi.new_sender();
        if ei.is_null() {
            // `ei_new_sender` failed: the fd was never transferred to
            // libei (`ei_setup_backend_fd` is what takes ownership), so it
            // must be released here to avoid leaking it.
            self.ffi.close_fd(fd);
            return Err(DesktopOutputError::Internal(
                "ei_new_sender returned NULL".to_string(),
            ));
        }
        let name = std::ffi::CString::new("touchpadctl output-probe (M6)")
            .expect("static client name has no NUL");
        self.ffi.configure_name(&ei, &name);
        let rc = self.ffi.setup_backend_fd(&ei, fd);
        if rc != 0 {
            // `ei_setup_backend_fd` takes ownership of the fd even on
            // failure (it stores it in the backend it tears down on
            // `ei_unref`), so we must NOT close it here. Dropping `ei`
            // releases the context (`ei_unref`) — exactly once.
            return Err(DesktopOutputError::Internal(format!(
                "ei_setup_backend_fd failed (errno {rc})"
            )));
        }
        self.ei = Some(ei);
        self.disconnected = false;
        Ok(())
    }

    fn wait_event(&mut self, timeout: Duration) -> Result<TransportEvent, DesktopOutputError> {
        if self.disconnected {
            return Ok(TransportEvent::Disconnected);
        }
        if self.ei.is_none() {
            return Err(DesktopOutputError::Internal(
                "wait_event before connect".to_string(),
            ));
        }

        // 1. Deliver already-fetched events first: a previous dispatch may
        // have queued several raw events, and the kernel fd is not readable
        // again until libei's internal queue is drained (M6 re-review R8).
        // Each event is mapped — its seat/device/resumed side effects
        // applied — exactly when it is popped for delivery (M6 re-review
        // R11).
        if let Some(event) = self.next_pending_mapped() {
            return Ok(self.deliver(event));
        }

        // 2. Drain libei's internal queue *before* polling: events queued by
        // an earlier dispatch are surfaced even when the fd reports no
        // readiness.
        self.drain_internal_queue();
        if let Some(event) = self.next_pending_mapped() {
            return Ok(self.deliver(event));
        }

        // 3. Only when nothing is queued, poll the fd.
        let fd = self.ffi.get_fd(self.ei.as_ref().expect("checked above"));
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let ready = match self.ffi.poll(fd, timeout_ms) {
            Ok(ready) => ready,
            Err(errno) if errno.kind() == std::io::ErrorKind::Interrupted => {
                // A signal interrupted the wait (the CLI installs a
                // SIGINT/SIGTERM handler without SA_RESTART); the driver
                // re-checks cancellation after a Timeout.
                return Ok(TransportEvent::Timeout);
            }
            Err(errno) => {
                return Err(DesktopOutputError::Internal(format!(
                    "poll on the libei fd failed: {errno}"
                )));
            }
        };
        if !ready {
            return Ok(TransportEvent::Timeout);
        }

        // 4. Dispatch (reads what arrived and flushes queued outgoing data),
        // then drain the **entire** internal queue into the native pending
        // queue so every event of this dispatch is surfaced even when the
        // following poll reports no readiness.
        self.ffi.dispatch(self.ei.as_ref().expect("checked above"));
        self.drain_internal_queue();
        Ok(self
            .next_pending_mapped()
            .map(|event| self.deliver(event))
            .unwrap_or(TransportEvent::Timeout))
    }

    fn pump(&mut self) -> Result<Vec<TransportEvent>, DesktopOutputError> {
        let mut events = Vec::new();
        // Always dispatch once before polling: libei flushes queued outgoing
        // data when dispatched (the backend fd is nonblocking, so a dispatch
        // with no incoming data returns immediately), so the pump flushes
        // even when the fd reports no readiness — the code and the flush
        // documentation agree (M6 re-review R8). This also processes data
        // that arrived since the last poll.
        if let Some(ei) = &self.ei {
            self.ffi.dispatch(ei);
        }
        loop {
            match self.wait_event(Duration::ZERO)? {
                TransportEvent::Timeout => break,
                // Disconnected is terminal: report it and stop (a repeated
                // `wait_event` would keep returning it).
                TransportEvent::Disconnected => {
                    events.push(TransportEvent::Disconnected);
                    break;
                }
                event => events.push(event),
            }
        }
        Ok(events)
    }

    fn bind_capabilities(
        &mut self,
        seat: SeatId,
        capabilities: u32,
    ) -> Result<(), DesktopOutputError> {
        let seat_handle = self
            .seats
            .iter()
            .find(|held| held.as_u64() == seat.0)
            .ok_or_else(|| {
                DesktopOutputError::Internal(format!("binding unknown seat {}", seat.0))
            })?;
        // Build the sentinel-terminated argument list from the set
        // capability bits (ascending), zero-padded to the fixed length the
        // FFI wrapper passes.
        let mut args = [0 as libc::c_int; 8];
        let mut index = 0;
        for cap in [
            EI_DEVICE_CAP_POINTER,
            EI_DEVICE_CAP_POINTER_ABSOLUTE,
            ffi::EI_DEVICE_CAP_KEYBOARD,
            ffi::EI_DEVICE_CAP_TOUCH,
            EI_DEVICE_CAP_SCROLL,
            EI_DEVICE_CAP_BUTTON,
        ] {
            if capabilities & cap as u32 != 0 {
                args[index] = cap;
                index += 1;
            }
        }
        self.ffi.seat_bind_capabilities(seat_handle, &args);
        Ok(())
    }

    fn start_emulating(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let device_handle = self.require_device(device)?;
        self.ffi.device_start_emulating(device_handle, sequence);
        Ok(())
    }

    fn pointer_motion(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        self.ffi.device_pointer_motion(device_handle, dx, dy);
        Ok(())
    }

    fn button(
        &mut self,
        device: DeviceId,
        button: u32,
        is_press: bool,
    ) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        self.ffi
            .device_button_button(device_handle, button, is_press);
        Ok(())
    }

    fn scroll_delta(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        self.ffi.device_scroll_delta(device_handle, dx, dy);
        Ok(())
    }

    fn scroll_stop(
        &mut self,
        device: DeviceId,
        stop_x: bool,
        stop_y: bool,
    ) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        self.ffi.device_scroll_stop(device_handle, stop_x, stop_y);
        Ok(())
    }

    fn frame(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        let ei = self.ei.as_ref().ok_or_else(|| {
            DesktopOutputError::Internal("frame without a connected context".to_string())
        })?;
        let now = self.ffi.now(ei);
        self.ffi.device_frame(device_handle, now);
        Ok(())
    }

    fn frame_at(&mut self, device: DeviceId, time_us: u64) -> Result<(), DesktopOutputError> {
        let device_handle = self.require_device(device)?;
        if self.ei.is_none() {
            return Err(DesktopOutputError::Internal(
                "frame without a connected context".to_string(),
            ));
        }
        self.ffi.device_frame(device_handle, time_us);
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), DesktopOutputError> {
        if self.disconnected && self.ei.is_none() {
            return Ok(());
        }
        self.disconnected = true;
        if let Some(ei) = &self.ei {
            self.ffi.disconnect(ei);
        }
        self.teardown();
        Ok(())
    }
}

impl<F: NativeFfi> NativeTransport<F> {
    fn device_capability_bits(&self, device: &EiDevice) -> u32 {
        let mut bits = 0;
        if self
            .ffi
            .device_has_capability(device, EI_DEVICE_CAP_POINTER)
        {
            bits |= EI_DEVICE_CAP_POINTER as u32;
        }
        if self.ffi.device_has_capability(device, EI_DEVICE_CAP_SCROLL) {
            bits |= EI_DEVICE_CAP_SCROLL as u32;
        }
        if self.ffi.device_has_capability(device, EI_DEVICE_CAP_BUTTON) {
            bits |= EI_DEVICE_CAP_BUTTON as u32;
        }
        bits
    }
}

#[cfg(test)]
mod tests {
    // The scripted FFI fake constructs raw libei handles (`from_raw`) to
    // exercise the real transport algorithm below raw FFI; the production
    // code of this module contains no `unsafe`.
    #![allow(unsafe_code)]

    use super::*;

    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::Arc;

    /// A stand-in for the libei unref functions: returns the pointer,
    /// exactly like the real `ei_*_unref` functions (used by the scripted
    /// FFI's fake handles; the one-time-release semantics themselves are
    /// proven by the `ffi` module's ownership tests).
    unsafe extern "C" fn fake_unref(ptr: *mut c_void) -> *mut c_void {
        ptr
    }

    /// The events the scripted FFI can produce.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ScriptedEvent {
        Connect,
        Disconnect,
        SeatAdded,
        SeatRemoved,
        DeviceAdded,
        DeviceRemoved,
        DevicePaused,
        DeviceResumed,
    }

    /// A scripted [`NativeFfi`] that mimics libei's **actual queue
    /// contract**: events enter the internal queue only via `dispatch`, and
    /// `get_event` pops them one at a time — plus a scripted `poll`
    /// readiness. This is the native-adapter seam *below* raw FFI: the real
    /// `NativeTransport` queue algorithm (deliver-pending-first,
    /// drain-before-poll, drain-after-dispatch, unconditional flush on pump)
    /// is exercised without a real library, fd, or emission (M6 re-review
    /// R8).
    #[derive(Debug)]
    struct ScriptedFfi {
        /// libei's internal event queue (`get_event` pops).
        queue: RefCell<VecDeque<ScriptedEvent>>,
        /// What the next `dispatch` reads off the fd into `queue`.
        next_batch: RefCell<VecDeque<ScriptedEvent>>,
        /// `poll` readiness.
        fd_ready: Cell<bool>,
        /// `dispatch` call count (the flush assertion).
        dispatches: Cell<usize>,
        /// `poll` call count (proves pending delivery without polling).
        polls: Cell<usize>,
        /// Pointer identity for live events.
        next_ptr: Cell<usize>,
        /// The event kind behind each live event pointer.
        kinds: RefCell<HashMap<usize, ScriptedEvent>>,
        /// The library guard pinned by every handle the fake creates (M6
        /// re-review R7): the host-process library, always loaded.
        lib: Arc<libloading::Library>,
        /// The context pointer the fake hands out.
        context_ptr: usize,
        /// The seat pointer scripted events refer to.
        seat_ptr: usize,
        /// The device pointer scripted events refer to.
        device_ptr: usize,
        /// Capabilities reported for the scripted device.
        device_capabilities: u32,
        /// Device type reported for the scripted device.
        device_type: i32,
        /// Number of emission calls (start_emulating/motion/button/scroll/
        /// frame).
        emissions: Cell<usize>,
        /// Timestamp supplied to the most recent device frame.
        last_frame_time_us: Cell<Option<u64>>,
        /// Number of disconnect calls.
        disconnects: Cell<usize>,
    }

    impl ScriptedFfi {
        fn new() -> Self {
            Self {
                queue: RefCell::new(VecDeque::new()),
                next_batch: RefCell::new(VecDeque::new()),
                fd_ready: Cell::new(false),
                dispatches: Cell::new(0),
                polls: Cell::new(0),
                next_ptr: Cell::new(0x1000),
                kinds: RefCell::new(HashMap::new()),
                lib: Arc::new(libloading::os::unix::Library::this().into()),
                context_ptr: 0xE1E1,
                seat_ptr: 0x5EA7,
                device_ptr: 0xDEA7_0001,
                device_capabilities: crate::sink::BIND_CAPABILITY_BITS,
                device_type: crate::ffi::EI_DEVICE_TYPE_VIRTUAL,
                emissions: Cell::new(0),
                last_frame_time_us: Cell::new(None),
                disconnects: Cell::new(0),
            }
        }
    }

    impl NativeFfi for ScriptedFfi {
        fn new_sender(&self) -> EiContext {
            // SAFETY: the fake hands out its canonical non-null context
            // pointer; the handle pins `self.lib` (M6 re-review R7).
            unsafe {
                EiContext::from_raw(
                    self.context_ptr as *mut c_void,
                    fake_unref,
                    Arc::clone(&self.lib),
                )
            }
        }

        fn configure_name(&self, _ei: &EiContext, _name: &std::ffi::CStr) {}

        fn setup_backend_fd(&self, _ei: &EiContext, _fd: i32) -> i32 {
            0
        }

        fn get_fd(&self, _ei: &EiContext) -> i32 {
            3
        }

        fn dispatch(&self, _ei: &EiContext) {
            // libei contract: dispatch reads whatever arrived on the fd into
            // the internal queue (and flushes outgoing data).
            self.dispatches.set(self.dispatches.get() + 1);
            let mut queue = self.queue.borrow_mut();
            let mut batch = self.next_batch.borrow_mut();
            queue.append(&mut batch);
        }

        fn get_event(&self, _ei: &EiContext) -> EiEvent {
            let mut queue = self.queue.borrow_mut();
            match queue.pop_front() {
                None => {
                    // SAFETY: a null event handle is the valid "no event"
                    // value.
                    unsafe {
                        EiEvent::from_raw(std::ptr::null_mut(), fake_unref, Arc::clone(&self.lib))
                    }
                }
                Some(kind) => {
                    let ptr = self.next_ptr.get();
                    self.next_ptr.set(ptr + 1);
                    self.kinds.borrow_mut().insert(ptr, kind);
                    // SAFETY: `ptr` is a unique fake event pointer the fake
                    // itself allocated and tracks in `kinds`.
                    unsafe {
                        EiEvent::from_raw(ptr as *mut c_void, fake_unref, Arc::clone(&self.lib))
                    }
                }
            }
        }

        fn event_get_type(&self, event: &EiEvent) -> i32 {
            let kind = self.kinds.borrow()[&(event.as_ptr() as usize)];
            match kind {
                ScriptedEvent::Connect => crate::ffi::EI_EVENT_CONNECT,
                ScriptedEvent::Disconnect => crate::ffi::EI_EVENT_DISCONNECT,
                ScriptedEvent::SeatAdded => crate::ffi::EI_EVENT_SEAT_ADDED,
                ScriptedEvent::SeatRemoved => crate::ffi::EI_EVENT_SEAT_REMOVED,
                ScriptedEvent::DeviceAdded => crate::ffi::EI_EVENT_DEVICE_ADDED,
                ScriptedEvent::DeviceRemoved => crate::ffi::EI_EVENT_DEVICE_REMOVED,
                ScriptedEvent::DevicePaused => crate::ffi::EI_EVENT_DEVICE_PAUSED,
                ScriptedEvent::DeviceResumed => crate::ffi::EI_EVENT_DEVICE_RESUMED,
            }
        }

        fn event_get_seat<'a>(&self, _event: &'a EiEvent) -> EiSeatRef<'a> {
            // SAFETY: the fake's canonical seat pointer is a live libei seat
            // borrowed from the event.
            unsafe { EiSeatRef::from_raw(self.seat_ptr as *mut c_void) }
        }

        fn seat_ref(&self, seat: &EiSeatRef<'_>) -> EiSeat {
            // SAFETY: the fake's seat pointer is ref-counted; the handle
            // pins `self.lib`.
            unsafe { EiSeat::from_raw(seat.as_ptr(), fake_unref, Arc::clone(&self.lib)) }
        }

        fn seat_bind_capabilities(&self, _seat: &EiSeat, _capabilities: &[i32; 8]) {}

        fn event_get_device<'a>(&self, _event: &'a EiEvent) -> EiDeviceRef<'a> {
            // SAFETY: the fake's canonical device pointer is a live libei
            // device borrowed from the event.
            unsafe { EiDeviceRef::from_raw(self.device_ptr as *mut c_void) }
        }

        fn device_ref(&self, device: &EiDeviceRef<'_>) -> EiDevice {
            // SAFETY: the fake's device pointer is ref-counted; the handle
            // pins `self.lib`.
            unsafe { EiDevice::from_raw(device.as_ptr(), fake_unref, Arc::clone(&self.lib)) }
        }

        fn device_has_capability(&self, _device: &EiDevice, capability: i32) -> bool {
            self.device_capabilities & capability as u32 != 0
        }

        fn device_get_type(&self, _device: &EiDevice) -> i32 {
            self.device_type
        }

        fn device_start_emulating(&self, _device: &EiDevice, _sequence: u32) {
            self.emissions.set(self.emissions.get() + 1);
        }

        fn device_pointer_motion(&self, _device: &EiDevice, _dx: f64, _dy: f64) {
            self.emissions.set(self.emissions.get() + 1);
        }

        fn device_button_button(&self, _device: &EiDevice, _button: u32, _is_press: bool) {
            self.emissions.set(self.emissions.get() + 1);
        }

        fn device_scroll_delta(&self, _device: &EiDevice, _dx: f64, _dy: f64) {
            self.emissions.set(self.emissions.get() + 1);
        }

        fn device_scroll_stop(&self, _device: &EiDevice, _stop_x: bool, _stop_y: bool) {
            self.emissions.set(self.emissions.get() + 1);
        }

        fn device_frame(&self, _device: &EiDevice, time_us: u64) {
            self.last_frame_time_us.set(Some(time_us));
            self.emissions.set(self.emissions.get() + 1);
        }

        fn now(&self, _ei: &EiContext) -> u64 {
            0
        }

        fn disconnect(&self, _ei: &EiContext) {
            self.disconnects.set(self.disconnects.get() + 1);
        }

        fn poll(&self, _fd: i32, _timeout_ms: i32) -> Result<bool, std::io::Error> {
            self.polls.set(self.polls.get() + 1);
            Ok(self.fd_ready.get())
        }

        fn close_fd(&self, _fd: i32) {}
    }

    /// The device/seat ids the scripted FFI hands out (raw fake pointers).
    fn scripted_seat(ffi: &ScriptedFfi) -> SeatId {
        SeatId(ffi.seat_ptr as u64)
    }

    fn scripted_device(ffi: &ScriptedFfi) -> DeviceId {
        DeviceId(ffi.device_ptr as u64)
    }

    /// `NativeTransport` is only ever constructed over the scripted FFI in
    /// tests — never over a real libei context — so no test can emit real
    /// desktop input.
    fn scripted_transport(ffi: ScriptedFfi) -> NativeTransport<ScriptedFfi> {
        let mut transport = NativeTransport::new(ffi);
        transport.connect(42).unwrap();
        transport
    }

    /// M6 re-review R8: two or more events produced by **one** `ei_dispatch`
    /// are all surfaced even when the following `poll` reports no readiness
    /// (the kernel fd is not readable again until libei's internal queue is
    /// drained). The second event must come from the native pending queue,
    /// without polling at all.
    #[test]
    fn wait_event_surfaces_every_event_from_one_dispatch_when_poll_goes_stale() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch
            .borrow_mut()
            .extend([ScriptedEvent::SeatAdded, ScriptedEvent::DeviceAdded]);
        let mut transport = scripted_transport(ffi);

        // First wait: nothing pending, nothing queued -> poll (ready) ->
        // dispatch moves BOTH events into the internal queue -> both are
        // drained into the native pending queue; the first is returned.
        let first = transport.wait_event(Duration::ZERO).unwrap();
        assert_eq!(
            first,
            TransportEvent::SeatAdded {
                seat: scripted_seat(&transport.ffi)
            }
        );
        assert_eq!(transport.ffi.polls.get(), 1);

        // The kernel fd now reports no readiness (the dispatch consumed the
        // data that made it readable)...
        transport.ffi.fd_ready.set(false);
        // ... but the second event from the SAME dispatch is still surfaced,
        // from the native pending queue, without polling at all.
        let second = transport.wait_event(Duration::ZERO).unwrap();
        assert_eq!(
            second,
            TransportEvent::DeviceAdded {
                device: scripted_device(&transport.ffi),
                capabilities: transport.ffi.device_capabilities,
                device_type: DeviceType::Virtual,
            }
        );
        assert_eq!(
            transport.ffi.polls.get(),
            1,
            "the pending queue must be consumed before polling again"
        );

        // Everything drained: poll (still not ready) -> Timeout.
        let timeout = transport.wait_event(Duration::ZERO).unwrap();
        assert_eq!(timeout, TransportEvent::Timeout);
        assert_eq!(transport.ffi.polls.get(), 2);
    }

    /// M6 re-review R8: `pump` must dispatch — flushing queued outgoing
    /// libei data — even when the fd reports no readiness, so the code and
    /// the documented flush semantics agree.
    #[test]
    fn pump_flushes_outgoing_data_even_when_the_fd_is_not_readable() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(false);
        let mut transport = scripted_transport(ffi);

        let events = transport.pump().unwrap();
        assert!(events.is_empty());
        assert_eq!(
            transport.ffi.dispatches.get(),
            1,
            "pump must dispatch (flush) once even with no readable fd"
        );
        assert_eq!(transport.ffi.polls.get(), 1);
    }

    /// M6 re-review R8: `pump` surfaces every event of one dispatch, in
    /// order, ending at the terminal disconnect.
    #[test]
    fn pump_drains_every_event_from_one_dispatch_in_order() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::SeatAdded,
            ScriptedEvent::SeatRemoved,
            ScriptedEvent::Disconnect,
        ]);
        let mut transport = scripted_transport(ffi);

        let events = transport.pump().unwrap();
        assert_eq!(
            events,
            vec![
                TransportEvent::SeatAdded {
                    seat: scripted_seat(&transport.ffi)
                },
                TransportEvent::SeatRemoved {
                    seat: scripted_seat(&transport.ffi)
                },
                TransportEvent::Disconnected,
            ]
        );
        // The seat that was added was also removed: bookkeeping is
        // consistent with the delivered event order.
        assert!(transport.seats.is_empty());
    }

    /// The full libei lifecycle (connect, seat/device add, resume, pause,
    /// remove, seat remove) maps in order through the real transport, and
    /// the seat/device/resumed bookkeeping stays consistent with the
    /// delivered events.
    #[test]
    fn wait_event_maps_the_full_lifecycle_in_order() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::Connect,
            ScriptedEvent::SeatAdded,
            ScriptedEvent::DeviceAdded,
            ScriptedEvent::DeviceResumed,
            ScriptedEvent::DevicePaused,
            ScriptedEvent::DeviceRemoved,
            ScriptedEvent::SeatRemoved,
        ]);
        let mut transport = scripted_transport(ffi);

        let events = transport.pump().unwrap();
        assert_eq!(
            events,
            vec![
                TransportEvent::Connected,
                TransportEvent::SeatAdded {
                    seat: scripted_seat(&transport.ffi)
                },
                TransportEvent::DeviceAdded {
                    device: scripted_device(&transport.ffi),
                    capabilities: transport.ffi.device_capabilities,
                    device_type: DeviceType::Virtual,
                },
                TransportEvent::DeviceResumed {
                    device: scripted_device(&transport.ffi)
                },
                TransportEvent::DevicePaused {
                    device: scripted_device(&transport.ffi)
                },
                TransportEvent::DeviceRemoved {
                    device: scripted_device(&transport.ffi)
                },
                TransportEvent::SeatRemoved {
                    seat: scripted_seat(&transport.ffi)
                },
            ]
        );
        assert!(transport.seats.is_empty(), "seat released");
        assert!(transport.devices.is_empty(), "device released");
        assert!(
            transport.resumed.is_empty(),
            "pause/removal cleared the resumed set"
        );
    }

    /// M6 re-review R11: a single dispatch containing `SeatAdded, SeatRemoved`
    /// leaves the seat **bindable** until the queued `SeatRemoved` is
    /// actually delivered. With the old map-at-fetch design the seat was
    /// removed from `seats` before the caller ever saw `SeatAdded`, so
    /// `bind_capabilities` failed during the handshake.
    #[test]
    fn seat_stays_bindable_until_a_queued_seat_removed_is_delivered() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch
            .borrow_mut()
            .extend([ScriptedEvent::SeatAdded, ScriptedEvent::SeatRemoved]);
        let mut transport = scripted_transport(ffi);
        let seat = scripted_seat(&transport.ffi);

        // SeatAdded is delivered first; the queued SeatRemoved has NOT been
        // delivered yet, so the seat must still be present and bindable.
        let first = transport.wait_event(Duration::ZERO).unwrap();
        assert_eq!(first, TransportEvent::SeatAdded { seat });
        transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .expect("the seat must stay bindable until SeatRemoved is delivered");

        // Only now is SeatRemoved delivered...
        let second = transport.wait_event(Duration::ZERO).unwrap();
        assert_eq!(second, TransportEvent::SeatRemoved { seat });
        assert!(transport.seats.is_empty(), "seat released at delivery");

        // ... and only after delivery is the seat no longer bindable.
        let error = transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .unwrap_err();
        assert!(matches!(error, DesktopOutputError::Internal(_)), "{error}");
    }

    /// M6 re-review R11: a `DeviceResumed` queued before a `DevicePaused` in
    /// the same dispatch stays usable — `start_emulating` and emission
    /// succeed — until the pause is actually delivered; after delivery the
    /// device is no longer resumed and emission is rejected.
    #[test]
    fn resumed_device_stays_usable_until_a_queued_pause_is_delivered() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::SeatAdded,
            ScriptedEvent::DeviceAdded,
            ScriptedEvent::DeviceResumed,
            ScriptedEvent::DevicePaused,
        ]);
        let mut transport = scripted_transport(ffi);
        let seat = scripted_seat(&transport.ffi);
        let device = scripted_device(&transport.ffi);

        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::SeatAdded { seat }
        );
        transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .unwrap();
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceAdded {
                device,
                capabilities: transport.ffi.device_capabilities,
                device_type: DeviceType::Virtual,
            }
        );

        // DeviceResumed is delivered; the queued DevicePaused has NOT been
        // delivered yet, so the device must still be resumed and usable —
        // `start_emulating` (which `handshake` calls here) must succeed.
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceResumed { device }
        );
        transport
            .start_emulating(device)
            .expect("the resumed device must be usable until DevicePaused is delivered");

        // Only now is the pause delivered...
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DevicePaused { device }
        );
        assert!(
            transport.resumed.is_empty(),
            "the pause cleared the resumed set at delivery"
        );

        // ... and only after delivery is emission rejected.
        let error = transport.start_emulating(device).unwrap_err();
        assert!(matches!(error, DesktopOutputError::Internal(_)), "{error}");
        let error = transport.pointer_motion(device, 1.0, 0.0).unwrap_err();
        assert!(matches!(error, DesktopOutputError::Internal(_)), "{error}");
    }

    #[test]
    fn frame_at_uses_supplied_source_timestamp() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::SeatAdded,
            ScriptedEvent::DeviceAdded,
            ScriptedEvent::DeviceResumed,
        ]);
        let mut transport = scripted_transport(ffi);
        let seat = scripted_seat(&transport.ffi);
        let device = scripted_device(&transport.ffi);

        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::SeatAdded { seat }
        );
        transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .unwrap();
        assert!(matches!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceAdded { .. }
        ));
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceResumed { device }
        );

        transport.frame_at(device, 987_654).unwrap();
        assert_eq!(transport.ffi.last_frame_time_us.get(), Some(987_654));
    }

    /// M6 re-review R11: the same delivery-order guarantee for a queued
    /// `DeviceRemoved` — the resumed device stays usable until the removal
    /// is delivered, then the device is gone and every emission is rejected.
    #[test]
    fn resumed_device_stays_usable_until_a_queued_removal_is_delivered() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::SeatAdded,
            ScriptedEvent::DeviceAdded,
            ScriptedEvent::DeviceResumed,
            ScriptedEvent::DeviceRemoved,
        ]);
        let mut transport = scripted_transport(ffi);
        let seat = scripted_seat(&transport.ffi);
        let device = scripted_device(&transport.ffi);

        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::SeatAdded { seat }
        );
        transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .unwrap();
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceAdded {
                device,
                capabilities: transport.ffi.device_capabilities,
                device_type: DeviceType::Virtual,
            }
        );
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceResumed { device }
        );
        transport
            .start_emulating(device)
            .expect("the resumed device must be usable until DeviceRemoved is delivered");

        // Only now is the removal delivered; the RAII device owner is
        // released exactly then (balancing the ref taken on add).
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceRemoved { device }
        );
        assert!(transport.devices.is_empty(), "device released at delivery");
        assert!(transport.resumed.is_empty(), "removal cleared resumed");

        // After delivery every emission through the device is rejected.
        let error = transport.start_emulating(device).unwrap_err();
        assert!(matches!(error, DesktopOutputError::Internal(_)), "{error}");
        let error = transport.button(device, 0x110, true).unwrap_err();
        assert!(matches!(error, DesktopOutputError::Internal(_)), "{error}");
    }

    /// M6 re-review R11: a queued `Disconnect` becomes terminal **only when
    /// delivered** — the events queued before it in the same dispatch are
    /// all delivered first (each still usable at its delivery point), and
    /// only then does every further `wait_event` return `Disconnected`.
    #[test]
    fn queued_disconnect_is_terminal_only_at_delivery_without_losing_prior_events() {
        let ffi = ScriptedFfi::new();
        ffi.fd_ready.set(true);
        ffi.next_batch.borrow_mut().extend([
            ScriptedEvent::SeatAdded,
            ScriptedEvent::DeviceAdded,
            ScriptedEvent::DeviceResumed,
            ScriptedEvent::Disconnect,
        ]);
        let mut transport = scripted_transport(ffi);
        let seat = scripted_seat(&transport.ffi);
        let device = scripted_device(&transport.ffi);

        // The three lifecycle events queued before the disconnect are
        // delivered first, each still usable at its delivery point.
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::SeatAdded { seat }
        );
        transport
            .bind_capabilities(seat, crate::sink::BIND_CAPABILITY_BITS)
            .expect("seat bindable before the queued disconnect is delivered");
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceAdded {
                device,
                capabilities: transport.ffi.device_capabilities,
                device_type: DeviceType::Virtual,
            }
        );
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::DeviceResumed { device }
        );
        transport
            .start_emulating(device)
            .expect("device usable before the queued disconnect is delivered");

        // The disconnect is terminal only once delivered...
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::Disconnected
        );
        assert!(transport.disconnected);

        // ... and stays terminal on every later wait.
        assert_eq!(
            transport.wait_event(Duration::ZERO).unwrap(),
            TransportEvent::Disconnected
        );
    }

    /// The libei device-type mapping distinguishes virtual (logical pixels)
    /// from physical (millimetres) devices.
    #[test]
    fn device_type_mapping_matches_libei() {
        assert_eq!(DeviceType::from_raw(1), DeviceType::Virtual);
        assert_eq!(DeviceType::from_raw(2), DeviceType::Physical);
        assert_eq!(DeviceType::from_raw(99), DeviceType::Other(99));
    }

    /// The transport id types are opaque u64 wrappers.
    #[test]
    fn device_and_seat_ids_are_opaque_u64_wrappers() {
        let id = DeviceId(0xdead_beef_cafe_f00d);
        assert_eq!(id.0, id.0);
        let seat = SeatId(0x1234_5678_9abc_def0);
        assert_eq!(seat.0, seat.0);
    }
}
