#![forbid(unsafe_code)]
//! The transport seam (M6 required outcome 5): what the adapter sends to and
//! receives from the EIS implementation, behind a trait so the full session
//! logic is deterministic-testable with a fake transport and no Wayland
//! desktop. The real implementation is the runtime-loaded libei sender
//! transport ([`crate::native_transport::NativeTransport`], Linux-only); the
//! fake is [`crate::fake::FakeTransport`].

use std::time::Duration;

use crate::error::DesktopOutputError;

/// Opaque identity of a libei seat (a raw libei pointer on the native
/// transport, a counter on the fake). The adapter only ever passes back ids
/// it received from the transport, so the raw-pointer encoding is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeatId(pub u64);

/// Opaque identity of a libei device. See [`SeatId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u64);

/// The libei device type (`ei_device_get_type`, `libei.h` 1.6).
///
/// The unit mapping of relative deltas depends on it: a **virtual** device
/// reports deltas in **logical pixels** (the M6 contract), while a
/// **physical** device reports them in **millimetres**. The adapter only
/// claims the logical-pixel mapping for virtual devices and rejects
/// physical devices before emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// `EI_DEVICE_TYPE_VIRTUAL` — relative deltas are logical pixels.
    Virtual,
    /// `EI_DEVICE_TYPE_PHYSICAL` — relative deltas are millimetres; not
    /// usable by the M6 logical-pixel contract.
    Physical,
    /// An unknown/forward-compatible device type value.
    Other(i32),
}

impl DeviceType {
    /// Maps a raw `ei_device_get_type` value.
    #[must_use]
    pub fn from_raw(value: i32) -> Self {
        match value {
            crate::ffi::EI_DEVICE_TYPE_VIRTUAL => Self::Virtual,
            crate::ffi::EI_DEVICE_TYPE_PHYSICAL => Self::Physical,
            other => Self::Other(other),
        }
    }
}

/// An event the transport delivers to the adapter (the libei sender event
/// set: `CONNECT`, `SEAT_ADDED/REMOVED`, `DEVICE_ADDED/REMOVED/PAUSED/
/// RESUMED`, `DISCONNECT`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    /// The EIS server approved the connection (`EI_EVENT_CONNECT`).
    Connected,
    /// A seat became available; the adapter must bind capabilities
    /// ([`Transport::bind_capabilities`]) so the server creates devices.
    SeatAdded {
        /// The seat id.
        seat: SeatId,
    },
    /// A device was added; `capabilities` is the raw libei capability bitmask
    /// the negotiated device exposes ([`crate::capabilities::libei_capability_bits`]),
    /// and `device_type` is its libei device type (virtual = logical pixels,
    /// physical = millimetres).
    DeviceAdded {
        /// The device id.
        device: DeviceId,
        /// Raw libei `EI_DEVICE_CAP_*` bits.
        capabilities: u32,
        /// The libei device type (`ei_device_get_type`).
        device_type: DeviceType,
    },
    /// The device was resumed: the adapter may
    /// [`Transport::start_emulating`] and emit through it.
    DeviceResumed {
        /// The device id.
        device: DeviceId,
    },
    /// The device was paused; emitting through it is discarded until the next
    /// resume.
    DevicePaused {
        /// The device id.
        device: DeviceId,
    },
    /// The device was removed.
    DeviceRemoved {
        /// The device id.
        device: DeviceId,
    },
    /// A seat was removed.
    SeatRemoved {
        /// The seat id.
        seat: SeatId,
    },
    /// The transport disconnected (server closed the connection, or an
    /// error). Terminal: no further events follow.
    Disconnected,
    /// No event arrived within the requested wait window.
    Timeout,
}

/// The transport seam: a libei **sender** client that binds pointer/button/
/// scroll capabilities and emits relative motion, buttons, and pixel-precise
/// scroll events into a frame per logical event.
///
/// Ordering contract (mirrors libei): `wait_event` returns server events in
/// order; emission calls are only valid after
/// [`Transport::start_emulating`] on a resumed device and must be followed
/// by [`Transport::frame`]; [`Transport::disconnect`] is idempotent and
/// terminal.
pub trait Transport {
    /// Connect to the EIS server over the socket fd obtained from the
    /// portal. The transport takes ownership of `fd` (the native
    /// implementation hands it to `ei_setup_backend_fd`, which closes it on
    /// teardown). Idempotent/terminal afterwards.
    fn connect(&mut self, fd: i32) -> Result<(), DesktopOutputError>;

    /// Wait for the next server event, at most `timeout`.
    ///
    /// [`TransportEvent::Timeout`] is returned when nothing arrives (the
    /// driver re-checks cancellation and keeps waiting); a real disconnect
    /// is reported as [`TransportEvent::Disconnected`] exactly once.
    fn wait_event(&mut self, timeout: Duration) -> Result<TransportEvent, DesktopOutputError>;

    /// Drains every server event that is **currently available without
    /// blocking** (nonblocking pump).
    ///
    /// The adapter calls this around logical emission frames so a device
    /// pause/removal or a server disconnect becomes an observed, structured
    /// result instead of stale local state (M6 re-review R3). The native
    /// implementation also **flushes queued outgoing libei data** by
    /// dispatching; a write-side failure is surfaced by libei as an
    /// `EI_EVENT_DISCONNECT`, which this pump returns as the terminal
    /// [`TransportEvent::Disconnected`] entry.
    fn pump(&mut self) -> Result<Vec<TransportEvent>, DesktopOutputError>;

    /// Bind the seat to the given raw libei capability bits (the adapter
    /// binds pointer/button/scroll). The server then creates devices for
    /// those capabilities.
    fn bind_capabilities(
        &mut self,
        seat: SeatId,
        capabilities: u32,
    ) -> Result<(), DesktopOutputError>;

    /// Notify the server that the device is about to start sending events
    /// (`ei_device_start_emulating`). Only valid on a resumed device.
    fn start_emulating(&mut self, device: DeviceId) -> Result<(), DesktopOutputError>;

    /// Emit a relative pointer motion (`ei_device_pointer_motion`, logical
    /// pixels).
    fn pointer_motion(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError>;

    /// Emit a button press/release (`ei_device_button_button`; button codes
    /// follow `linux/input-event-codes.h`, e.g. `BTN_LEFT` 0x110,
    /// `BTN_RIGHT` 0x111).
    fn button(
        &mut self,
        device: DeviceId,
        button: u32,
        is_press: bool,
    ) -> Result<(), DesktopOutputError>;

    /// Emit a pixel-precise smooth scroll delta
    /// (`ei_device_scroll_delta`).
    fn scroll_delta(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError>;

    /// Emit a scroll stop for the given axes (`ei_device_scroll_stop`).
    fn scroll_stop(
        &mut self,
        device: DeviceId,
        stop_x: bool,
        stop_y: bool,
    ) -> Result<(), DesktopOutputError>;

    /// Close the current logical event frame (`ei_device_frame`); must be
    /// called after every group of emission calls. The timestamp is the
    /// transport's monotonic clock (µs).
    fn frame(&mut self, device: DeviceId) -> Result<(), DesktopOutputError>;

    /// Close a logical event frame using a source-provided monotonic
    /// timestamp in microseconds. Transports that cannot preserve source
    /// timing may keep the historical clock-at-send behavior by inheriting
    /// this default implementation.
    fn frame_at(&mut self, device: DeviceId, _time_us: u64) -> Result<(), DesktopOutputError> {
        self.frame(device)
    }

    /// Disconnect from the EIS implementation. Idempotent; terminal. On the
    /// native transport this also releases the compositor-side state
    /// (buttons/keys/scroll are reset when the connection closes).
    fn disconnect(&mut self) -> Result<(), DesktopOutputError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The raw-pointer id encoding must round-trip without truncation on
    /// 64-bit platforms (native transport encodes raw pointers as ids).
    #[test]
    fn device_and_seat_ids_round_trip_as_opaque_values() {
        let seat = SeatId(0x1234_5678_9abc_def0);
        let device = DeviceId(0x0fed_cba9_8765_4321);
        assert_eq!(SeatId(seat.0).0, seat.0);
        assert_eq!(DeviceId(device.0).0, device.0);
    }
}
