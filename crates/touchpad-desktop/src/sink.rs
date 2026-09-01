#![forbid(unsafe_code)]
//! The portal/libei session lifecycle and the typed
//! [`touchpad_core::OutputSink`] adapter (M6 required outcomes 1–3).
//!
//! # Lifecycle
//!
//! ```text
//! Disconnected ──prepare()──▶ Authorizing ──start ok──▶ Ready
//!     ▲                         │ (portal dialog)          │ ConnectToEIS
//!     │                         ▼                          ▼
//!     │                     refusal/cancel            libei handshake
//!     │                         │                          │
//!     └────────────── Stopped ◀─┴────── Stopping ◀── Emulating ◀┘
//!                          ▲          (release_all)
//!                          └── any active state
//!     Interrupted ── the EIS server paused/removed the active device,
//!                    removed its seat, or disconnected mid-emission: output
//!                    is rejected; release_all still closes the session.
//!     Fatal ── terminal state reached when a cleanup step itself fails
//! ```
//!
//! * `prepare()` performs the **output preparation and authorization** —
//!   portal session, device selection, user authorization, EIS connection,
//!   capability negotiation, device resume — and must complete before any
//!   future `EVIOCGRAB` (PHASE2_PLAN.md §3.1; M6 itself never grabs). The
//!   handshake is cancellation-aware ([`prepare_cancellable`]); the blocking
//!   portal waits are bounded and their delay before cleanup is documented.
//! * `submit()` emits only in [`SessionState::Emulating`], only for
//!   capabilities the negotiated device actually exposes, and tracks the
//!   held button/key/scroll state. Around every logical emission frame the
//!   transport is **pumped** (nonblocking): a server-side device pause,
//!   device/seat removal, or disconnect transitions the sink out of
//!   `Emulating`, rejects subsequent output, and becomes a structured
//!   failure instead of stale local state (M6 re-review R3).
//! * `release_all()` is idempotent and runs on **every** exit path —
//!   normal shutdown, fatal shutdown, partial send failure, server
//!   interruption, and fallback `Drop` — so no path can leave a logically
//!   held button or an open scroll lifecycle. It preserves the primary
//!   failure and the cleanup diagnostics.
//!
//! # Partial-send honesty
//!
//! A failed `submit` is reported as a failure and is **not** tracked as
//! held. Because libei queues events and flushes them on dispatch (which the
//! pump performs), a send failure surfaces as a transport error or a
//! disconnect event; the compositor-side state is reset by the disconnect
//! that `release_all` performs, so the tracked state and the wire state
//! cannot silently diverge.

use std::time::{Duration, Instant};

use touchpad_core::{
    Monotonic, MouseButton, OutputError, OutputEvent, OutputFrameError, OutputSink,
};

use crate::capabilities::{libei_capability_bits, Capability, OutputCapabilities};
use crate::error::DesktopOutputError;
use crate::held::HeldState;
use crate::portal::{device_types, Portal, PortalSession};
use crate::transport::{DeviceId, DeviceType, SeatId, Transport, TransportEvent};

/// The session lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No portal session exists yet.
    Disconnected,
    /// The portal session is being created and authorized (user dialog).
    Authorizing,
    /// The portal session is started; the EIS/libei connection is being
    /// established.
    Ready,
    /// A device is resumed and emission is allowed.
    Emulating,
    /// The EIS server paused/removed the active device, removed its seat, or
    /// disconnected **after** a successful handshake: emission is rejected;
    /// [`release_all`](Self::release_all) still closes the session (M6
    /// re-review R3).
    Interrupted,
    /// Shutdown in progress.
    Stopping,
    /// Terminal, clean: nothing held, connection closed, session closed.
    Stopped,
    /// Terminal, failed: a cleanup step could not complete.
    Fatal,
}

/// The libei device capability bits the adapter binds on every seat
/// (pointer + button + scroll — the M6 output contract). Touch is
/// deliberately never bound (no virtual touchpad, no raw contacts).
pub const BIND_CAPABILITY_BITS: u32 =
    libei_capability_bits::POINTER | libei_capability_bits::BUTTON | libei_capability_bits::SCROLL;

/// The maximum time the EIS handshake (connect → seat → device → resume)
/// may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// The per-wait granularity of the handshake loop.
pub const HANDSHAKE_WAIT: Duration = Duration::from_millis(500);

/// Linux input event codes for the supported buttons
/// (`linux/input-event-codes.h`): `BTN_LEFT` 0x110, `BTN_RIGHT` 0x111,
/// `BTN_MIDDLE` 0x112.
pub mod button_codes {
    /// `BTN_LEFT`.
    pub const BTN_LEFT: u32 = 0x110;
    /// `BTN_RIGHT`.
    pub const BTN_RIGHT: u32 = 0x111;
    /// `BTN_MIDDLE`.
    pub const BTN_MIDDLE: u32 = 0x112;
}

/// The portal/libei output adapter: implements
/// [`touchpad_core::OutputSink`] over a [`Portal`] + [`Transport`] pair.
#[derive(Debug)]
pub struct PortalOutputSink<P: Portal, T: Transport> {
    portal: P,
    transport: T,
    state: SessionState,
    session: Option<PortalSession>,
    seat: Option<SeatId>,
    device: Option<DeviceId>,
    capabilities: OutputCapabilities,
    held: HeldState,
    /// The detailed cleanup failure preserved from the last `release_all`.
    cleanup_error: Option<DesktopOutputError>,
    /// The structured server-side interruption observed by the post-handshake
    /// pump (device pause/removal, seat removal, disconnect), taken by the
    /// caller that reports it so the failure keeps its structured category
    /// (M6 re-review R3).
    interruption: Option<DesktopOutputError>,
    /// The EIS handshake deadline (injectable for fast tests).
    handshake_timeout: Duration,
}

impl<P: Portal, T: Transport> PortalOutputSink<P, T> {
    /// Creates a disconnected sink.
    #[must_use]
    pub fn new(portal: P, transport: T) -> Self {
        Self {
            portal,
            transport,
            state: SessionState::Disconnected,
            session: None,
            seat: None,
            device: None,
            capabilities: OutputCapabilities::NONE,
            held: HeldState::new(),
            cleanup_error: None,
            interruption: None,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    /// Overrides the EIS handshake deadline (tests use a short deadline so
    /// the timeout path is exercised quickly; the production default is
    /// [`HANDSHAKE_TIMEOUT`]).
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// The current lifecycle state.
    #[must_use]
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// The negotiated capabilities (valid after a successful
    /// [`prepare`](Self::prepare)).
    #[must_use]
    pub fn capabilities(&self) -> OutputCapabilities {
        self.capabilities
    }

    /// The portal session handle, if a session was created.
    #[must_use]
    pub fn session(&self) -> Option<&PortalSession> {
        self.session.as_ref()
    }

    /// The detailed cleanup failure preserved by the last `release_all`, if
    /// any (consumed by the caller that reports it).
    #[must_use]
    pub fn take_cleanup_error(&mut self) -> Option<DesktopOutputError> {
        self.cleanup_error.take()
    }

    /// The structured server-side interruption (device pause/removal, seat
    /// removal, or disconnect) observed by the post-handshake pump, if any —
    /// consumed by the caller that reports it, so the failure keeps its
    /// structured category instead of being flattened into a generic message
    /// (M6 re-review R3).
    #[must_use]
    pub fn take_server_interruption(&mut self) -> Option<DesktopOutputError> {
        self.interruption.take()
    }

    /// Performs the output preparation and authorization:
    ///
    /// 1. `CreateSession` → `SelectDevices(pointer)` → `Start`
    ///    (authorization dialog; refusal/cancel become structured errors);
    /// 2. `ConnectToEIS` → the EIS socket fd;
    /// 3. libei connection + handshake: bind pointer/button/scroll on the
    ///    seat, wait for a useful **virtual** device to be added and resumed,
    ///    and start emulating;
    /// 4. state → [`SessionState::Emulating`].
    ///
    /// Returns the negotiated capabilities. On any failure the sink is left
    /// in a state where [`release_all`](Self::release_all) is still
    /// well-defined (it closes the session/connection), and the primary
    /// failure is preserved together with any cleanup failure (M6 re-review
    /// R4).
    pub fn prepare(&mut self) -> Result<OutputCapabilities, DesktopOutputError> {
        self.prepare_cancellable(&|| false)
    }

    /// Same as [`prepare`](Self::prepare), but aborts with
    /// [`DesktopOutputError::Cancelled`] when `cancelled` turns true while
    /// the EIS handshake is being driven.
    ///
    /// The blocking portal waits (`CreateSession`/`SelectDevices`/`Start`/
    /// `ConnectToEIS`) are bounded D-Bus calls (15s each, 120s for the
    /// authorization dialog in `portal_zbus`): a signal during those waits
    /// delays the ordered cleanup by at most the remaining bounded wait, and
    /// is observed at the next cancellation check. The handshake itself is
    /// polled every [`HANDSHAKE_WAIT`] and checks the hook, so a signal
    /// during it aborts promptly and the partial session is released.
    pub fn prepare_cancellable(
        &mut self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<OutputCapabilities, DesktopOutputError> {
        if self.state != SessionState::Disconnected {
            return Err(DesktopOutputError::Internal(format!(
                "prepare() called in state {:?}",
                self.state
            )));
        }
        if cancelled() {
            return Err(DesktopOutputError::Cancelled);
        }
        self.state = SessionState::Authorizing;

        // 1. Portal session + authorization.
        let session = self.portal.create_session()?;
        self.session = Some(session.clone());
        let step = (|| {
            self.portal
                .select_devices(&session, device_types::POINTER)?;
            self.portal.start(&session)
        })();
        if let Err(error) = step {
            return Err(self.cleanup_after_prepare_failure(error));
        }
        if cancelled() {
            // The user aborted while the authorization dialog was up (the
            // bounded Start wait returned without a response): release the
            // authorized session.
            return Err(self.cleanup_after_prepare_failure(DesktopOutputError::Cancelled));
        }
        self.state = SessionState::Ready;

        // 2. EIS fd.
        let eis_fd = match self.portal.connect_to_eis(&session) {
            Ok(fd) => fd,
            Err(error) => return Err(self.cleanup_after_prepare_failure(error)),
        };

        // 3. libei connection + handshake.
        if let Err(error) = self.transport.connect(eis_fd.0) {
            return Err(self.cleanup_after_prepare_failure(error));
        }
        match self.handshake(Some(cancelled)) {
            Ok(capabilities) => Ok(capabilities),
            // Any handshake failure still leaves no live session.
            Err(error) => Err(self.cleanup_after_prepare_failure(error)),
        }
    }

    /// Returns the primary preparation failure, wrapped in a
    /// [`DesktopOutputError::PrepareFailed`] composite when the cleanup of
    /// the partially-prepared session also failed — the primary cause (and
    /// its category/exit precedence) is preserved, and the cleanup
    /// diagnostics are carried to the caller instead of being discarded
    /// (M6 re-review R4).
    fn cleanup_after_prepare_failure(&mut self, primary: DesktopOutputError) -> DesktopOutputError {
        match self.release_all_detailed() {
            Ok(()) => primary,
            Err(cleanup) => DesktopOutputError::PrepareFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            },
        }
    }

    /// Drives the EIS handshake until the device is resumed and emulating
    /// starts, or a structured failure/timeout occurs. `cancelled` (when
    /// provided) is checked every wait so a signal aborts the handshake
    /// promptly.
    fn handshake(
        &mut self,
        cancelled: Option<&dyn Fn() -> bool>,
    ) -> Result<OutputCapabilities, DesktopOutputError> {
        let started = Instant::now();
        let mut candidate: Option<DeviceId> = None;
        // A useful (non-empty capability) device that was rejected because
        // it is physical: remembered so the timeout can report the honest
        // reason instead of a generic "no device" (M6 cleanup: the real
        // device type is queried; physical devices report millimetres, not
        // logical pixels).
        let mut saw_useful_physical = false;
        loop {
            if let Some(cancelled) = cancelled {
                if cancelled() {
                    return Err(DesktopOutputError::Cancelled);
                }
            }
            let event = self.transport.wait_event(HANDSHAKE_WAIT)?;
            match event {
                TransportEvent::Connected => {}
                TransportEvent::SeatAdded { seat } => {
                    self.seat = Some(seat);
                    self.transport
                        .bind_capabilities(seat, BIND_CAPABILITY_BITS)?;
                }
                TransportEvent::DeviceAdded {
                    device,
                    capabilities,
                    device_type,
                } => {
                    let caps = OutputCapabilities::from_device_capability_bits(capabilities);
                    if caps.is_empty() {
                        continue;
                    }
                    if device_type != DeviceType::Virtual {
                        // A physical device reports relative deltas in
                        // millimetres; M6 must not claim the logical-pixel
                        // unit mapping for it. Wait for a virtual device (the
                        // KWin EIS server presents a single virtual pointer).
                        saw_useful_physical = true;
                        continue;
                    }
                    if candidate.is_none() {
                        // M6 uses the first useful device (the KWin EIS
                        // server presents a single virtual pointer carrying
                        // pointer/button/scroll); capability splits across
                        // multiple devices are reported as missing on the
                        // chosen device rather than silently combined.
                        self.capabilities = caps;
                        candidate = Some(device);
                    }
                }
                TransportEvent::DeviceResumed { device } if candidate == Some(device) => {
                    self.device = Some(device);
                    self.transport.start_emulating(device)?;
                    self.state = SessionState::Emulating;
                    return Ok(self.capabilities);
                }
                TransportEvent::DeviceResumed { .. } => {}
                TransportEvent::DevicePaused { .. } => {}
                TransportEvent::DeviceRemoved { device } if candidate == Some(device) => {
                    candidate = None;
                    self.capabilities = OutputCapabilities::NONE;
                }
                TransportEvent::DeviceRemoved { .. } => {}
                TransportEvent::SeatRemoved { seat } if Some(seat) == self.seat => {
                    self.seat = None;
                }
                TransportEvent::SeatRemoved { .. } => {}
                TransportEvent::Disconnected => {
                    return Err(DesktopOutputError::TransportDisconnected(
                        "EIS server disconnected during handshake".to_string(),
                    ));
                }
                TransportEvent::Timeout => {
                    if started.elapsed() > self.handshake_timeout {
                        if saw_useful_physical && candidate.is_none() {
                            return Err(DesktopOutputError::CapabilityMissing(
                                "the EIS server only exposed a physical device; relative deltas \
                                 for physical devices are in millimetres, so the logical-pixel \
                                 unit mapping cannot be claimed (a virtual device is required)"
                                    .to_string(),
                            ));
                        }
                        return Err(DesktopOutputError::Timeout(
                            "EIS handshake (no emulatable device within 15s)".to_string(),
                        ));
                    }
                }
            }
        }
    }

    /// Validates an event against the state, the negotiated capabilities,
    /// and the held-state lifecycle, **without mutating** anything.
    fn validate(&self, event: &OutputEvent) -> Result<(), OutputError> {
        if self.state != SessionState::Emulating {
            return Err(OutputError::Unavailable(format!(
                "session is not emulating (state {:?})",
                self.state
            )));
        }
        match event {
            OutputEvent::PointerMove { .. } => {
                self.require_capability(Capability::RelativePointer)?;
            }
            OutputEvent::ButtonDown(button) | OutputEvent::ButtonUp(button) => {
                self.validate_button(*button)?;
            }
            OutputEvent::ScrollBegin | OutputEvent::ScrollDelta { .. } | OutputEvent::ScrollEnd => {
                self.require_capability(Capability::PixelScroll)?;
            }
            OutputEvent::KeyDown(_) | OutputEvent::KeyUp(_) => {
                return Err(OutputError::Unavailable(
                    "keyboard capability is not negotiated by M6".to_string(),
                ));
            }
            OutputEvent::DesktopAction(_) => {
                return Err(OutputError::Unavailable(
                    "desktop actions are not emitted by the M6 pointer device".to_string(),
                ));
            }
            OutputEvent::ContinuousGesture(_) => {
                return Err(OutputError::Unavailable(
                    "continuous gesture semantics require a dedicated desktop adapter".to_string(),
                ));
            }
        }
        // Lifecycle validation (pure — does not mutate).
        self.held.validate(event)?;
        Ok(())
    }

    fn validate_button(&self, button: MouseButton) -> Result<(), OutputError> {
        match button {
            MouseButton::Left => self.require_capability(Capability::PrimaryButton),
            MouseButton::Right => self.require_capability(Capability::SecondaryButton),
            MouseButton::Middle => self.require_capability(Capability::MiddleButton),
            MouseButton::Other(_) => Err(OutputError::Unavailable(
                "only primary/secondary/middle buttons are negotiated by M6".to_string(),
            )),
            // Non-exhaustive enum from touchpad-core: future variants are
            // not emitted by the M6 adapter.
            _ => Err(OutputError::Unavailable(
                "only primary/secondary buttons are negotiated by M6".to_string(),
            )),
        }
    }

    fn require_capability(&self, capability: Capability) -> Result<(), OutputError> {
        if self.capabilities.supports(capability) {
            Ok(())
        } else {
            Err(OutputError::Unavailable(format!(
                "the negotiated device does not expose {capability:?}"
            )))
        }
    }

    /// Sends one semantic event's wire calls without closing a libei frame.
    /// Returns whether the event emitted a wire request. Local lifecycle
    /// markers (`ScrollBegin`, a zero delta, or a no-axis `ScrollEnd`) return
    /// `false` and therefore require no frame commit.
    fn send_unframed(&mut self, event: &OutputEvent) -> Result<bool, DesktopOutputError> {
        let device = self.device.ok_or_else(|| {
            DesktopOutputError::Internal("emitting without a resumed device".to_string())
        })?;
        match event {
            OutputEvent::PointerMove { dx, dy } => {
                self.transport.pointer_motion(
                    device,
                    f64::from(dx.as_px()),
                    f64::from(dy.as_px()),
                )?;
            }
            OutputEvent::ButtonDown(button) | OutputEvent::ButtonUp(button) => {
                let (code, is_press) = match event {
                    OutputEvent::ButtonDown(_) => (button_code(*button), true),
                    _ => (button_code(*button), false),
                };
                self.transport.button(device, code, is_press)?;
            }
            OutputEvent::ScrollBegin => {
                // No libei wire event and no frame: the first nonzero delta
                // starts the scroll on the server side. Return early so no
                // frame is emitted for a pure state marker.
                return Ok(false);
            }
            OutputEvent::ScrollDelta { dx, dy } => {
                if dx.as_px() == 0.0 && dy.as_px() == 0.0 {
                    // A fully-zero delta is a local lifecycle marker: nothing
                    // moved, so nothing reaches the wire and no frame is
                    // closed (M6 re-review R5/R9: zero deltas never activate
                    // an axis and never start server-side scrolling).
                    return Ok(false);
                }
                self.transport.scroll_delta(
                    device,
                    f64::from(dx.as_px()),
                    f64::from(dy.as_px()),
                )?;
            }
            OutputEvent::ScrollEnd => {
                // Stop exactly the axes that received nonzero deltas in this
                // interaction (libei tracks scrolling per axis; M6 re-review
                // R5).
                let (stop_x, stop_y) = self.held.scroll_stop_axes();
                if !(stop_x || stop_y) {
                    // No axis received a nonzero delta: a no-axis
                    // `scroll_stop(false, false)` is documented by libei as a
                    // client logic bug. `ScrollEnd` is a local lifecycle
                    // marker with **no wire stop and no frame**, analogous to
                    // `ScrollBegin` (M6 re-review R9).
                    return Ok(false);
                }
                self.transport.scroll_stop(device, stop_x, stop_y)?;
            }
            OutputEvent::KeyDown(_)
            | OutputEvent::KeyUp(_)
            | OutputEvent::DesktopAction(_)
            | OutputEvent::ContinuousGesture(_) => {
                return Err(DesktopOutputError::Internal(
                    "unreachable: validate() rejects these events".to_string(),
                ));
            }
        }
        Ok(true)
    }

    /// Sends one event and closes its libei logical frame. Historical
    /// single-event submission keeps this behavior; `submit_frame` may keep a
    /// drag button edge and its owned motion in one shared hardware frame.
    fn send(&mut self, event: &OutputEvent) -> Result<(), DesktopOutputError> {
        if !self.send_unframed(event)? {
            return Ok(());
        }
        let device = self.device.ok_or_else(|| {
            DesktopOutputError::Internal("framing without a resumed device".to_string())
        })?;
        self.transport.frame(device)
    }

    fn send_at(
        &mut self,
        event: &OutputEvent,
        timestamp: Monotonic,
    ) -> Result<(), DesktopOutputError> {
        if !self.send_unframed(event)? {
            return Ok(());
        }
        let device = self.device.ok_or_else(|| {
            DesktopOutputError::Internal("framing without a resumed device".to_string())
        })?;
        self.transport
            .frame_at(device, timestamp.as_nanos() / 1_000)
    }

    /// Commits exactly two wire-bearing semantic events as one libei logical
    /// hardware frame. Used only for drag ownership edges paired with pointer
    /// motion (`Down -> Move` or `Move -> Up`). A tap pulse (`Down -> Up`) is
    /// deliberately never routed here because libei allows only one request
    /// per button per frame and the compositor must observe both edges.
    fn submit_drag_pair(
        &mut self,
        first: &OutputEvent,
        second: &OutputEvent,
        timestamp: Option<Monotonic>,
    ) -> Result<(), OutputFrameError> {
        for (index, event) in [first, second].into_iter().enumerate() {
            if let Err(primary) = self.validate(event) {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: 0,
                    primary,
                });
            }
        }

        if let Err(error) = self.pump_transport() {
            return Err(OutputFrameError {
                failed_index: 0,
                accepted_prefix: 0,
                primary: OutputError::Io(error.to_string()),
            });
        }
        if let Err(error) = self.send_unframed(first) {
            return Err(OutputFrameError {
                failed_index: 0,
                accepted_prefix: 0,
                primary: OutputError::Io(error.to_string()),
            });
        }
        if let Err(error) = self.send_unframed(second) {
            return Err(OutputFrameError {
                failed_index: 1,
                accepted_prefix: 0,
                primary: OutputError::Io(error.to_string()),
            });
        }
        let Some(device) = self.device else {
            return Err(OutputFrameError {
                failed_index: 1,
                accepted_prefix: 0,
                primary: OutputError::Io("libei drag frame lost its active device".to_string()),
            });
        };
        let frame_result = match timestamp {
            Some(timestamp) => self
                .transport
                .frame_at(device, timestamp.as_nanos() / 1_000),
            None => self.transport.frame(device),
        };
        if let Err(error) = frame_result {
            return Err(OutputFrameError {
                failed_index: 1,
                accepted_prefix: 0,
                primary: OutputError::Io(error.to_string()),
            });
        }
        if let Err(error) = self.pump_transport() {
            return Err(OutputFrameError {
                failed_index: 1,
                accepted_prefix: 0,
                primary: OutputError::Io(error.to_string()),
            });
        }

        // Validation was performed against the pre-frame state. These two
        // supported pair shapes cannot invalidate one another's lifecycle,
        // so recording after the shared wire commit is infallible.
        let first_record = self.held.record(first);
        debug_assert!(first_record.is_ok());
        let second_record = self.held.record(second);
        debug_assert!(second_record.is_ok());
        Ok(())
    }

    /// Nonblocking pump of the transport around emission frames: observes
    /// server events that arrived since the last pump (device
    /// pause/removal, seat removal, disconnect) and transitions the sink out
    /// of `Emulating` — rejecting further output — when the active
    /// device/seat is affected (M6 re-review R3). On the native transport
    /// this also flushes queued outgoing libei data; write-side errors
    /// surface as the terminal `Disconnected` event.
    fn pump_transport(&mut self) -> Result<(), DesktopOutputError> {
        let events = self.transport.pump()?;
        self.apply_server_events(events)
    }

    fn apply_server_events(
        &mut self,
        events: Vec<TransportEvent>,
    ) -> Result<(), DesktopOutputError> {
        let mut interruption: Option<DesktopOutputError> = None;
        for event in events {
            match event {
                TransportEvent::DevicePaused { device } if Some(device) == self.device => {
                    interruption.get_or_insert_with(|| {
                        DesktopOutputError::DevicePaused(format!(
                            "the EIS device {} was paused by the server",
                            device.0
                        ))
                    });
                }
                TransportEvent::DeviceRemoved { device } if Some(device) == self.device => {
                    interruption.get_or_insert_with(|| {
                        DesktopOutputError::TransportDisconnected(format!(
                            "the EIS device {} was removed by the server",
                            device.0
                        ))
                    });
                }
                TransportEvent::SeatRemoved { seat } if Some(seat) == self.seat => {
                    interruption.get_or_insert_with(|| {
                        DesktopOutputError::TransportDisconnected(format!(
                            "the EIS seat {} was removed by the server",
                            seat.0
                        ))
                    });
                }
                TransportEvent::Disconnected => {
                    interruption.get_or_insert_with(|| {
                        DesktopOutputError::TransportDisconnected(
                            "the EIS server disconnected".to_string(),
                        )
                    });
                }
                _ => {}
            }
        }
        if let Some(error) = interruption {
            self.state = SessionState::Interrupted;
            self.device = None;
            self.seat = None;
            self.interruption = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    /// Detailed, idempotent `release_all`: releases every held button and
    /// the open scroll lifecycle, disconnects the transport (the
    /// compositor-side backstop that resets any remaining state), and
    /// closes the portal session. Returns the first failure with all
    /// diagnostics preserved; the failure is also stored for
    /// [`take_cleanup_error`](Self::take_cleanup_error).
    pub fn release_all_detailed(&mut self) -> Result<(), DesktopOutputError> {
        if self.state == SessionState::Stopped {
            return Ok(());
        }
        if self.state == SessionState::Disconnected && self.held.is_clean() {
            return Ok(());
        }
        // When the server already paused/removed the active device, removed
        // its seat, or disconnected, there is no live device to release
        // through: the release sends are skipped and the disconnect below is
        // the compositor-side reset (the backstop) (M6 re-review R3).
        let interrupted = self.state == SessionState::Interrupted;
        self.state = SessionState::Stopping;

        let mut failures = Vec::new();

        // 1. Release held button/key/scroll state through the transport.
        if !interrupted {
            for event in self.held.release_events() {
                match self.send(&event) {
                    Ok(()) => {
                        // The release reached the wire; update the tracker.
                        let _ = self.held.record(&event);
                    }
                    Err(error) => failures.push(error),
                }
            }
        }

        // 2. Disconnect the transport — the compositor resets all emulated
        //    state when the EIS connection closes, which is the backstop
        //    that guarantees no logically held state survives even when a
        //    release send failed above.
        let disconnect_result = self.transport.disconnect();
        let disconnected_ok = disconnect_result.is_ok();
        if let Err(error) = disconnect_result {
            failures.push(error);
        }
        self.device = None;

        // 3. Close the portal session (best-effort cleanup of the handle).
        if let Some(session) = self.session.take() {
            if let Err(error) = self.portal.close_session(&session) {
                failures.push(error);
            }
        }

        self.held.clear();
        self.interruption = None;

        // The disconnect is the compositor-side state reset: when it
        // succeeded the session is neutral even if an individual release
        // send failed (reported below); when it failed the session is
        // terminal-failed.
        self.state = if disconnected_ok {
            SessionState::Stopped
        } else {
            SessionState::Fatal
        };

        if failures.is_empty() {
            self.cleanup_error = None;
            Ok(())
        } else {
            let error = DesktopOutputError::ReleaseFailed(compose_failures(&failures));
            self.cleanup_error = Some(error.clone());
            Err(error)
        }
    }
}

/// Maps a `MouseButton` to the Linux input event code.
fn button_code(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => button_codes::BTN_LEFT,
        MouseButton::Right => button_codes::BTN_RIGHT,
        // Middle is accepted through the generic libei BUTTON capability;
        // Other buttons are rejected by `validate_button` before `send`.
        MouseButton::Middle => button_codes::BTN_MIDDLE,
        MouseButton::Other(code) => u32::from(code),
        // Non-exhaustive enum from touchpad-core: future variants are not
        // emitted by the M6 adapter (validate_button rejects them).
        _ => 0,
    }
}

/// Joins failure messages into one composite string without losing any
/// diagnostic.
fn compose_failures(failures: &[DesktopOutputError]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

impl<P: Portal, T: Transport> OutputSink for PortalOutputSink<P, T> {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        // Validate (state, capability, lifecycle) without mutating.
        self.validate(&event)?;
        // Pump before sending: a pause/removal/disconnect that already
        // happened is observed before any new wire event is emitted (M6
        // re-review R3).
        self.pump_transport()
            .map_err(|error| OutputError::Io(error.to_string()))?;
        // Send; only a fully successful send is tracked as held.
        self.send(&event)
            .map_err(|error| OutputError::Io(error.to_string()))?;
        // Pump after the logical frame: observes server state changes
        // triggered by the emission and flushes queued outgoing libei data
        // (write-side errors surface as a disconnect here). If the server
        // interrupted the session, the event reached the wire but is not
        // tracked as held — the disconnect performed by `release_all` resets
        // the compositor-side state (the backstop).
        self.pump_transport()
            .map_err(|error| OutputError::Io(error.to_string()))?;
        // `validate` passed and nothing mutated between validation and the
        // send, so the commit cannot fail.
        let _ = self.held.record(&event);
        Ok(())
    }

    fn submit_frame(&mut self, events: &[OutputEvent]) -> Result<(), OutputFrameError> {
        // Keep the ownership edge and its first/final relative motion in the
        // same EIS hardware frame. KWin therefore observes one coherent
        // pointer state transition instead of a button frame followed by a
        // second motion frame (or vice versa). Do not batch click pulses:
        // Down+Up for one button must remain two observable frames.
        if events.len() == 2
            && matches!(
                (&events[0], &events[1]),
                (OutputEvent::ButtonDown(_), OutputEvent::PointerMove { .. })
                    | (OutputEvent::PointerMove { .. }, OutputEvent::ButtonUp(_))
            )
        {
            return self.submit_drag_pair(&events[0], &events[1], None);
        }

        for (index, event) in events.iter().enumerate() {
            if let Err(primary) = self.submit(event.clone()) {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary,
                });
            }
        }
        Ok(())
    }

    fn submit_frame_at(
        &mut self,
        timestamp: Monotonic,
        events: &[OutputEvent],
    ) -> Result<(), OutputFrameError> {
        if events.len() == 2
            && matches!(
                (&events[0], &events[1]),
                (OutputEvent::ButtonDown(_), OutputEvent::PointerMove { .. })
                    | (OutputEvent::PointerMove { .. }, OutputEvent::ButtonUp(_))
            )
        {
            return self.submit_drag_pair(&events[0], &events[1], Some(timestamp));
        }

        for (index, event) in events.iter().enumerate() {
            if let Err(primary) = self.validate(event) {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary,
                });
            }
            if let Err(error) = self.pump_transport() {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary: OutputError::Io(error.to_string()),
                });
            }
            if let Err(error) = self.send_at(event, timestamp) {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary: OutputError::Io(error.to_string()),
                });
            }
            if let Err(error) = self.pump_transport() {
                return Err(OutputFrameError {
                    failed_index: index,
                    accepted_prefix: index,
                    primary: OutputError::Io(error.to_string()),
                });
            }
            let record = self.held.record(event);
            debug_assert!(record.is_ok());
        }
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        match self.release_all_detailed() {
            Ok(()) => Ok(()),
            Err(error) => Err(OutputError::Fatal(error.to_string())),
        }
    }
}

impl<P: Portal, T: Transport> Drop for PortalOutputSink<P, T> {
    /// Best-effort fallback: a sink dropped while still live must not leave
    /// a logically held state. The explicit [`release_all`](Self::release_all)
    /// is the primary path; this is the fallback for early returns and
    /// unwinds.
    fn drop(&mut self) {
        let _ = self.release_all_detailed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;

    use crate::fake::{FakePortal, FakePortalStep, FakeTransport, FakeWireCall};
    use crate::portal::EisFd;
    use touchpad_core::output::KeyId;

    fn full_caps() -> OutputCapabilities {
        OutputCapabilities::from_device_capability_bits(BIND_CAPABILITY_BITS)
    }

    fn happy_handshake() -> (FakePortal, FakeTransport, DeviceId) {
        let portal = FakePortal::success();
        let device = DeviceId(7);
        let transport = FakeTransport::happy_handshake(device);
        (portal, transport, device)
    }

    fn prepared_sink() -> (PortalOutputSink<FakePortal, FakeTransport>, DeviceId) {
        let (portal, transport, device) = happy_handshake();
        let mut sink = PortalOutputSink::new(portal, transport);
        sink.prepare().unwrap();
        (sink, device)
    }

    fn px(value: f32) -> touchpad_core::LogicalPixels {
        touchpad_core::LogicalPixels::try_new(value).unwrap()
    }

    #[test]
    fn prepare_reaches_emulating_with_negotiated_capabilities() {
        let (sink, _) = prepared_sink();
        assert_eq!(sink.state(), SessionState::Emulating);
        assert_eq!(sink.capabilities(), full_caps());
        assert!(sink.session().is_some());
    }

    #[test]
    fn prepare_rejects_when_called_twice() {
        let (mut sink, _) = prepared_sink();
        assert!(matches!(
            sink.prepare(),
            Err(DesktopOutputError::Internal(_))
        ));
    }

    #[test]
    fn prepare_reports_cancelled_authorization_and_still_releases() {
        let mut portal = FakePortal::success();
        portal.start_behavior = Some(DesktopOutputError::AuthorizationCancelled);
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut sink = PortalOutputSink::new(portal, transport);
        let error = sink.prepare().unwrap_err();
        assert_eq!(error, DesktopOutputError::AuthorizationCancelled);
        // The failed prepare must not leave a live session: release ran and
        // closed the session.
        assert!(sink.session().is_none());
        assert!(matches!(
            sink.state(),
            SessionState::Stopped | SessionState::Fatal
        ));
    }

    #[test]
    fn prepare_reports_transport_disconnect_during_handshake() {
        let portal = FakePortal::success();
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        transport.events.clear();
        transport.events.push_back(TransportEvent::Connected);
        transport.events.push_back(TransportEvent::Disconnected);
        let mut sink = PortalOutputSink::new(portal, transport);
        let error = sink.prepare().unwrap_err();
        assert!(
            matches!(error, DesktopOutputError::TransportDisconnected(_)),
            "{error}"
        );
    }

    #[test]
    fn prepare_times_out_without_a_useful_device() {
        let portal = FakePortal::success();
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        transport.events.clear();
        // A seat but never a device: the handshake times out.
        transport.events.push_back(TransportEvent::Connected);
        transport.events.push_back(TransportEvent::SeatAdded {
            seat: crate::transport::SeatId(1),
        });
        let mut sink = PortalOutputSink::new(portal, transport)
            .with_handshake_timeout(Duration::from_millis(50));
        let error = sink.prepare().unwrap_err();
        assert!(matches!(error, DesktopOutputError::Timeout(_)), "{error}");
    }

    #[test]
    fn submit_is_rejected_before_emulating() {
        let portal = FakePortal::success();
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut sink = PortalOutputSink::new(portal, transport);
        // Not prepared: no emission allowed.
        let error = sink
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .unwrap_err();
        assert!(matches!(error, OutputError::Unavailable(_)), "{error}");
    }

    #[test]
    fn submit_emits_motion_button_and_scroll_in_order() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::PointerMove {
            dx: px(10.0),
            dy: px(0.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ButtonUp(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-120.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ScrollEnd).unwrap();

        let log = sink.transport.log().to_vec();
        let expected = vec![
            FakeWireCall::Connect(42),
            FakeWireCall::BindCapabilities {
                seat: crate::transport::SeatId(1),
                capabilities: BIND_CAPABILITY_BITS,
            },
            FakeWireCall::StartEmulating { device },
            FakeWireCall::PointerMotion {
                device,
                dx: 10.0,
                dy: 0.0,
            },
            FakeWireCall::Frame { device },
            FakeWireCall::Button {
                device,
                button: button_codes::BTN_LEFT,
                is_press: true,
            },
            FakeWireCall::Frame { device },
            FakeWireCall::Button {
                device,
                button: button_codes::BTN_LEFT,
                is_press: false,
            },
            FakeWireCall::Frame { device },
            FakeWireCall::ScrollDelta {
                device,
                dx: 0.0,
                dy: -120.0,
            },
            FakeWireCall::Frame { device },
            // M6 re-review R5: only the y axis received a nonzero delta, so
            // only y is stopped.
            FakeWireCall::ScrollStop {
                device,
                stop_x: false,
                stop_y: true,
            },
            FakeWireCall::Frame { device },
        ];
        assert_eq!(log, expected);
    }

    #[test]
    fn middle_button_uses_generic_button_capability_and_btn_middle_code() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Middle))
            .unwrap();
        sink.submit(OutputEvent::ButtonUp(MouseButton::Middle))
            .unwrap();

        let log = sink.transport.log();
        assert!(log.windows(4).any(|window| {
            matches!(
                window,
                [
                    FakeWireCall::Button {
                        device: down_device,
                        button: button_codes::BTN_MIDDLE,
                        is_press: true,
                    },
                    FakeWireCall::Frame { device: down_frame },
                    FakeWireCall::Button {
                        device: up_device,
                        button: button_codes::BTN_MIDDLE,
                        is_press: false,
                    },
                    FakeWireCall::Frame { device: up_frame },
                ] if *down_device == device
                    && *down_frame == device
                    && *up_device == device
                    && *up_frame == device
            )
        }));
    }

    #[test]
    fn submit_frame_keeps_drag_press_and_first_motion_in_one_libei_frame() {
        let (mut sink, device) = prepared_sink();
        let before = sink.transport.log().len();

        sink.submit_frame(&[
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::PointerMove {
                dx: px(12.0),
                dy: px(-7.0),
            },
        ])
        .unwrap();

        assert_eq!(
            &sink.transport.log()[before..],
            &[
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: true,
                },
                FakeWireCall::PointerMotion {
                    device,
                    dx: 12.0,
                    dy: -7.0,
                },
                FakeWireCall::Frame { device },
            ]
        );
    }

    #[test]
    fn submit_frame_at_preserves_source_monotonic_timestamp() {
        let (mut sink, device) = prepared_sink();
        let before = sink.transport.log().len();

        sink.submit_frame_at(
            Monotonic::from_nanos(123_456_789),
            &[OutputEvent::PointerMove {
                dx: px(3.0),
                dy: px(-2.0),
            }],
        )
        .unwrap();

        assert_eq!(
            &sink.transport.log()[before..],
            &[
                FakeWireCall::PointerMotion {
                    device,
                    dx: 3.0,
                    dy: -2.0,
                },
                FakeWireCall::FrameAt {
                    device,
                    time_us: 123_456,
                },
            ]
        );
    }

    #[test]
    fn submit_frame_keeps_final_motion_and_drag_release_in_one_libei_frame() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        let before = sink.transport.log().len();

        sink.submit_frame(&[
            OutputEvent::PointerMove {
                dx: px(-4.0),
                dy: px(3.0),
            },
            OutputEvent::ButtonUp(MouseButton::Left),
        ])
        .unwrap();

        assert_eq!(
            &sink.transport.log()[before..],
            &[
                FakeWireCall::PointerMotion {
                    device,
                    dx: -4.0,
                    dy: 3.0,
                },
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: false,
                },
                FakeWireCall::Frame { device },
            ]
        );
    }

    #[test]
    fn submit_frame_keeps_tap_down_up_as_two_libei_frames() {
        let (mut sink, device) = prepared_sink();
        let before = sink.transport.log().len();

        sink.submit_frame(&[
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::ButtonUp(MouseButton::Left),
        ])
        .unwrap();

        assert_eq!(
            &sink.transport.log()[before..],
            &[
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: true,
                },
                FakeWireCall::Frame { device },
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: false,
                },
                FakeWireCall::Frame { device },
            ]
        );
    }

    #[test]
    fn consecutive_drag_starts_each_begin_from_a_fresh_libei_frame() {
        let (mut sink, device) = prepared_sink();
        let before = sink.transport.log().len();

        sink.submit_frame(&[
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::PointerMove {
                dx: px(9.0),
                dy: px(-5.0),
            },
        ])
        .unwrap();
        sink.submit_frame(&[OutputEvent::ButtonUp(MouseButton::Left)])
            .unwrap();
        sink.submit_frame(&[
            OutputEvent::ButtonDown(MouseButton::Left),
            OutputEvent::PointerMove {
                dx: px(-6.0),
                dy: px(4.0),
            },
        ])
        .unwrap();

        assert_eq!(
            &sink.transport.log()[before..],
            &[
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: true,
                },
                FakeWireCall::PointerMotion {
                    device,
                    dx: 9.0,
                    dy: -5.0,
                },
                FakeWireCall::Frame { device },
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: false,
                },
                FakeWireCall::Frame { device },
                FakeWireCall::Button {
                    device,
                    button: button_codes::BTN_LEFT,
                    is_press: true,
                },
                FakeWireCall::PointerMotion {
                    device,
                    dx: -6.0,
                    dy: 4.0,
                },
                FakeWireCall::Frame { device },
            ]
        );
    }

    #[test]
    fn scroll_begin_has_no_wire_event() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().all(|call| !matches!(
                call,
                FakeWireCall::ScrollDelta { .. } | FakeWireCall::ScrollStop { .. }
            )),
            "ScrollBegin must not touch the wire: {log:?}"
        );
    }

    #[test]
    fn capability_missing_events_are_rejected() {
        let portal = FakePortal::success();
        // A pointer-only device: no scroll, no buttons.
        let device = DeviceId(7);
        let transport = FakeTransport::happy_handshake_with_caps(device, 1 << 0);
        let mut sink = PortalOutputSink::new(portal, transport);
        sink.prepare().unwrap();
        assert!(sink.capabilities().supports(Capability::RelativePointer));
        assert!(!sink.capabilities().supports(Capability::PixelScroll));

        sink.submit(OutputEvent::PointerMove {
            dx: px(1.0),
            dy: px(0.0),
        })
        .unwrap();
        assert!(matches!(
            sink.submit(OutputEvent::ScrollBegin),
            Err(OutputError::Unavailable(_))
        ));
        assert!(matches!(
            sink.submit(OutputEvent::ButtonDown(MouseButton::Left)),
            Err(OutputError::Unavailable(_))
        ));
    }

    #[test]
    fn keyboard_and_desktop_actions_are_not_negotiated() {
        let (mut sink, _) = prepared_sink();
        assert!(matches!(
            sink.submit(OutputEvent::KeyDown(KeyId::new(1))),
            Err(OutputError::Unavailable(_))
        ));
        assert!(matches!(
            sink.submit(OutputEvent::DesktopAction(
                touchpad_core::DesktopAction::ShowDesktop
            )),
            Err(OutputError::Unavailable(_))
        ));
    }

    #[test]
    fn partial_send_failure_is_honest_and_not_tracked() {
        let (mut sink, _) = prepared_sink();
        sink.transport.send_error = Some(DesktopOutputError::TransportDisconnected(
            "injected".to_string(),
        ));
        // The failed press is reported as a failure...
        let error = sink
            .submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        // ... and is NOT tracked as held: release emits nothing for it and
        // the session shuts down cleanly (the disconnect resets the
        // compositor-side state).
        sink.release_all().unwrap();
        assert!(sink.held.is_clean());
        assert_eq!(sink.state(), SessionState::Stopped);
    }

    #[test]
    fn release_all_is_idempotent_and_returns_to_neutral() {
        let (mut sink, _) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-10.0),
        })
        .unwrap();
        assert!(!sink.held.is_clean());

        sink.release_all().unwrap();
        assert!(sink.held.is_clean());
        assert_eq!(sink.state(), SessionState::Stopped);

        // Second release is a no-op success.
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
    }

    #[test]
    fn release_all_emits_releases_before_disconnect_and_close() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Right))
            .unwrap();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-120.0),
        })
        .unwrap();

        sink.release_all().unwrap();
        let log = sink.transport.log().to_vec();
        let release_start = log
            .iter()
            .position(|call| {
                matches!(
                    call,
                    FakeWireCall::Button {
                        is_press: false,
                        ..
                    } | FakeWireCall::ScrollStop { .. }
                )
            })
            .expect("release events in the log");
        let disconnect = log
            .iter()
            .position(|call| matches!(call, FakeWireCall::Disconnect))
            .expect("disconnect in the log");
        assert!(
            release_start < disconnect,
            "releases must precede the disconnect: {log:?}"
        );
        // Both buttons and the scroll stop were released.
        let ups: Vec<_> = log
            .iter()
            .filter(|call| {
                matches!(
                    call,
                    FakeWireCall::Button {
                        is_press: false,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(ups.len(), 2, "{log:?}");
        assert!(
            log.iter()
                .any(|call| matches!(call, FakeWireCall::ScrollStop { .. })),
            "{log:?}"
        );
        // The portal session was closed.
        assert_eq!(sink.portal.close_calls, 1);
        assert!(sink.session().is_none());
        // Every release was routed through the negotiated device.
        assert!(
            log.iter().all(|call| match call {
                FakeWireCall::Button { device: d, .. }
                | FakeWireCall::ScrollStop { device: d, .. }
                | FakeWireCall::Frame { device: d } => *d == device,
                _ => true,
            }),
            "{log:?}"
        );
    }

    #[test]
    fn fallback_drop_releases_held_state() {
        let (mut sink, _) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        // Drop without an explicit release_all: the fallback must release.
        drop(sink);
    }

    #[test]
    fn release_failure_is_preserved_and_reported() {
        let (mut sink, _) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        // Fail the disconnect: release can no longer guarantee neutrality.
        sink.transport.disconnect_error = Some(DesktopOutputError::TransportDisconnected(
            "injected".to_string(),
        ));
        let error = sink.release_all_detailed().unwrap_err();
        assert!(
            matches!(error, DesktopOutputError::ReleaseFailed(_)),
            "{error}"
        );
        assert!(matches!(sink.state(), SessionState::Fatal));
        // The detailed error is also retrievable.
        assert!(sink.take_cleanup_error().is_some());
    }

    // ── M6 re-review R4: prepare failures must preserve the cleanup failure ──

    /// Fault-injects every preparation stage (SelectDevices, Start,
    /// ConnectToEIS, transport connect, handshake) and asserts that a
    /// failing `close_session` during the cleanup is carried to the caller as
    /// a composite `PrepareFailed` whose **primary** category is preserved.
    #[test]
    fn prepare_failure_with_cleanup_failure_is_composite_and_preserves_primary() {
        /// One fault-injection case: the stage name, the injection, and the
        /// expected primary failure.
        type PrepareCase = (
            &'static str,
            Box<dyn Fn(&mut FakePortal, &mut FakeTransport)>,
            DesktopOutputError,
        );
        let cases: Vec<PrepareCase> = vec![
            (
                "select_devices",
                Box::new(|portal, _transport| {
                    portal.fail_step = Some((
                        FakePortalStep::SelectDevices,
                        DesktopOutputError::PortalUnavailable("no types".into()),
                    ));
                }),
                DesktopOutputError::PortalUnavailable("no types".into()),
            ),
            (
                "start",
                Box::new(|portal, _transport| {
                    portal.fail_step = Some((
                        FakePortalStep::Start,
                        DesktopOutputError::AuthorizationRefused {
                            response: 2,
                            message: "nope".into(),
                        },
                    ));
                }),
                DesktopOutputError::AuthorizationRefused {
                    response: 2,
                    message: "nope".into(),
                },
            ),
            (
                "connect_to_eis",
                Box::new(|portal, _transport| {
                    portal.fail_step = Some((
                        FakePortalStep::ConnectToEis,
                        DesktopOutputError::PortalUnavailable("no fd".into()),
                    ));
                }),
                DesktopOutputError::PortalUnavailable("no fd".into()),
            ),
            (
                "transport_connect",
                Box::new(|_portal, transport| {
                    transport.connect_error = Some(DesktopOutputError::Internal(
                        "ei_new_sender returned NULL".into(),
                    ));
                }),
                DesktopOutputError::Internal("ei_new_sender returned NULL".into()),
            ),
            (
                "handshake_disconnect",
                Box::new(|_portal, transport| {
                    transport.events.clear();
                    transport.events.push_back(TransportEvent::Connected);
                    transport.events.push_back(TransportEvent::Disconnected);
                }),
                DesktopOutputError::TransportDisconnected(
                    "EIS server disconnected during handshake".into(),
                ),
            ),
        ];

        for (name, inject, expected_primary) in cases {
            let mut portal = FakePortal::success();
            let mut transport = FakeTransport::happy_handshake(DeviceId(7));
            inject(&mut portal, &mut transport);
            // Also make the cleanup's `close_session` fail (independently of
            // the injected stage failure).
            portal.close_error = Some(DesktopOutputError::PortalUnavailable("close failed".into()));
            let mut sink = PortalOutputSink::new(portal, transport);
            let error = sink.prepare().unwrap_err();
            let message = format!("{error}");
            assert!(
                matches!(
                    &error,
                    DesktopOutputError::PrepareFailed { primary, cleanup }
                        if **primary == expected_primary
                            && matches!(&**cleanup, DesktopOutputError::ReleaseFailed(_))
                ),
                "{name}: expected PrepareFailed with primary {expected_primary:?}, got {error:?}"
            );
            // The composite's category (and therefore exit code) is the
            // primary's — not flattened into the cleanup's.
            assert_eq!(error.category(), expected_primary.category(), "{name}");
            assert!(message.contains("prepare failed"), "{name}");
            assert!(message.contains("cleanup also failed"), "{name}");
            // The cleanup diagnostics are also retrievable from the sink.
            assert!(sink.take_cleanup_error().is_some(), "{name}");
        }
    }

    /// Without a cleanup failure, a prepare failure stays the plain primary
    /// error (no composite wrapper).
    #[test]
    fn prepare_failure_without_cleanup_failure_is_the_plain_primary() {
        let mut portal = FakePortal::success();
        portal.fail_step = Some((
            FakePortalStep::ConnectToEis,
            DesktopOutputError::PortalUnavailable("no fd".into()),
        ));
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut sink = PortalOutputSink::new(portal, transport);
        let error = sink.prepare().unwrap_err();
        assert!(
            matches!(&error, DesktopOutputError::PortalUnavailable(_)),
            "{error:?}"
        );
        assert!(sink.session().is_none());
        assert_eq!(sink.state(), SessionState::Stopped);
    }

    /// The same composite applies when the transport disconnect fails during
    /// the cleanup of a failed handshake (both cleanup steps fail).
    #[test]
    fn prepare_handshake_failure_with_transport_disconnect_failure_is_composite() {
        let portal = FakePortal::success();
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        transport.events.clear();
        transport.events.push_back(TransportEvent::Connected);
        transport.events.push_back(TransportEvent::Disconnected);
        transport.disconnect_error = Some(DesktopOutputError::TransportDisconnected(
            "disconnect injected".into(),
        ));
        let mut sink = PortalOutputSink::new(portal, transport);
        let error = sink.prepare().unwrap_err();
        assert!(
            matches!(
                &error,
                DesktopOutputError::PrepareFailed { primary, cleanup }
                    if matches!(&**primary, DesktopOutputError::TransportDisconnected(_))
                        && matches!(&**cleanup, DesktopOutputError::ReleaseFailed(_))
            ),
            "{error:?}"
        );
        assert_eq!(
            error.category(),
            DesktopOutputError::TransportDisconnected("x".into()).category()
        );
        assert!(matches!(sink.state(), SessionState::Fatal));
    }

    // ── M6 re-review R3: post-handshake server events (pump) ──

    /// A device pause after a successful handshake transitions the sink out
    /// of `Emulating`, rejects output, emits **no** wire event for the
    /// rejected submit, and still cleans up.
    #[test]
    fn server_pause_after_handshake_rejects_output_and_cleans_up() {
        let (mut sink, device) = prepared_sink();
        let mut transport = sink.transport.clone();
        // Script a pause of the active device, observed at the next pump.
        transport
            .events
            .push_back(TransportEvent::DevicePaused { device });
        // Replace the transport inside the sink with the scripted clone.
        sink.transport = transport;
        let before = sink.transport.log().len();

        let error = sink
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        // No wire event was emitted for the rejected submit.
        assert_eq!(
            sink.transport.log().len(),
            before,
            "no wire event after pause"
        );
        // The structured failure is retrievable.
        assert!(
            matches!(
                sink.take_server_interruption(),
                Some(DesktopOutputError::DevicePaused(_))
            ),
            "structured pause failure"
        );
        // Subsequent output is rejected (not Emulating).
        assert!(matches!(
            sink.submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            }),
            Err(OutputError::Unavailable(_))
        ));
        // Cleanup still runs: no release sends after the pause — the only new
        // wire call is the disconnect (the compositor-side backstop), and the
        // session is closed.
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
        assert_eq!(sink.portal.close_calls, 1);
        let after = sink.transport.log().to_vec();
        assert_eq!(
            &after[before..],
            &[FakeWireCall::Disconnect],
            "only the disconnect may follow a pause: {after:?}"
        );
    }

    /// A server disconnect after a successful handshake becomes the
    /// structured `TransportDisconnected` failure; no later wire event is
    /// emitted and cleanup still runs.
    #[test]
    fn server_disconnect_after_handshake_rejects_output_and_cleans_up() {
        let (mut sink, _device) = prepared_sink();
        let mut transport = sink.transport.clone();
        transport.events.push_back(TransportEvent::Disconnected);
        sink.transport = transport;
        let before = sink.transport.log().len();

        let error = sink
            .submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        assert_eq!(sink.transport.log().len(), before);
        assert!(matches!(
            sink.take_server_interruption(),
            Some(DesktopOutputError::TransportDisconnected(_))
        ));
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
        assert_eq!(sink.portal.close_calls, 1);
    }

    /// A device removal after a successful handshake becomes a structured
    /// failure; no later wire event is emitted.
    #[test]
    fn server_removal_after_handshake_rejects_output_and_cleans_up() {
        let (mut sink, device) = prepared_sink();
        let mut transport = sink.transport.clone();
        transport
            .events
            .push_back(TransportEvent::DeviceRemoved { device });
        sink.transport = transport;
        let before = sink.transport.log().len();

        let error = sink
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        assert_eq!(sink.transport.log().len(), before);
        assert!(matches!(
            sink.take_server_interruption(),
            Some(DesktopOutputError::TransportDisconnected(_))
        ));
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
        assert_eq!(sink.portal.close_calls, 1);
    }

    /// A seat removal of the active seat after a successful handshake also
    /// interrupts the session.
    #[test]
    fn server_seat_removal_after_handshake_interrupts() {
        let (mut sink, _device) = prepared_sink();
        let mut transport = sink.transport.clone();
        transport.events.push_back(TransportEvent::SeatRemoved {
            seat: crate::transport::SeatId(1),
        });
        sink.transport = transport;
        let before = sink.transport.log().len();

        let error = sink
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        assert_eq!(sink.transport.log().len(), before);
        assert!(matches!(
            sink.take_server_interruption(),
            Some(DesktopOutputError::TransportDisconnected(_))
        ));
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
    }

    /// A pause of a *different* device does not interrupt the session: the
    /// active device keeps emitting.
    #[test]
    fn pause_of_another_device_is_ignored() {
        let (mut sink, _device) = prepared_sink();
        let mut transport = sink.transport.clone();
        transport.events.push_back(TransportEvent::DevicePaused {
            device: DeviceId(999),
        });
        sink.transport = transport;
        sink.submit(OutputEvent::PointerMove {
            dx: px(1.0),
            dy: px(0.0),
        })
        .unwrap();
        assert_eq!(sink.state(), SessionState::Emulating);
        assert!(sink.take_server_interruption().is_none());
        // The motion went through.
        assert!(
            sink.transport
                .log()
                .iter()
                .any(|call| matches!(call, FakeWireCall::PointerMotion { .. })),
            "motion emitted"
        );
    }

    /// The pump drains every currently-available event in order and stops at
    /// the first `Timeout` (the end of the currently-available batch); a
    /// terminal disconnect ends the pump (no infinite loop).
    #[test]
    fn pump_drains_all_queued_server_events() {
        let mut transport = FakeTransport::new();
        transport.events.push_back(TransportEvent::DeviceResumed {
            device: DeviceId(7),
        });
        transport.events.push_back(TransportEvent::Connected); // drained in the same pump
        let events = transport.pump().unwrap();
        assert_eq!(
            events,
            vec![
                TransportEvent::DeviceResumed {
                    device: DeviceId(7)
                },
                TransportEvent::Connected,
            ]
        );
        // A Timeout (end of the currently-available batch) stops the pump.
        let mut transport = FakeTransport::new();
        transport.events.push_back(TransportEvent::Connected);
        transport.events.push_back(TransportEvent::Timeout);
        transport.events.push_back(TransportEvent::SeatAdded {
            seat: crate::transport::SeatId(1),
        });
        let events = transport.pump().unwrap();
        assert_eq!(events, vec![TransportEvent::Connected]);
        // A terminal disconnect ends the pump.
        let mut transport = FakeTransport::new();
        transport.events.push_back(TransportEvent::Disconnected);
        let events = transport.pump().unwrap();
        assert_eq!(events, vec![TransportEvent::Disconnected]);
    }

    // ── M6 re-review R3/R2: cancellation-aware handshake ──

    /// A cancellation requested *during* the handshake aborts `prepare`
    /// promptly with `Cancelled` and still releases the partial session.
    #[test]
    fn cancellation_during_handshake_aborts_and_releases() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let portal = FakePortal::success();
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        // The cancellation flag flips on the first handshake wait (the
        // portal steps completed, so the abort is observed by the handshake
        // loop, not by the entry check).
        let flag = Arc::new(AtomicBool::new(false));
        let transport = CancelOnHandshakeWait {
            inner: transport,
            flag: Arc::clone(&flag),
        };
        let mut sink = PortalOutputSink::new(portal, transport)
            .with_handshake_timeout(Duration::from_secs(60));
        let cancelled = || flag.load(Ordering::Relaxed);
        let error = sink.prepare_cancellable(&cancelled).unwrap_err();
        assert_eq!(error, DesktopOutputError::Cancelled);
        assert!(sink.session().is_none(), "session released");
        assert!(matches!(
            sink.state(),
            SessionState::Stopped | SessionState::Fatal
        ));
    }

    /// Cancellation before the handshake starts is observed immediately.
    #[test]
    fn cancellation_before_prepare_is_observed() {
        let portal = FakePortal::success();
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut sink = PortalOutputSink::new(portal, transport);
        let cancelled = || true;
        let error = sink.prepare_cancellable(&cancelled).unwrap_err();
        assert_eq!(error, DesktopOutputError::Cancelled);
        assert_eq!(sink.state(), SessionState::Disconnected);
    }

    /// A cancellation observed after authorization but before emission
    /// releases the authorized session: the portal's `start` (authorization)
    /// succeeds and then the cancellation flag flips, so the post-start check
    /// aborts with the session already authorized.
    #[test]
    fn cancellation_after_authorization_releases_the_session() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let flag = Arc::new(AtomicBool::new(false));
        let portal = CancelAfterAuthorize {
            inner: FakePortal::success(),
            flag: Arc::clone(&flag),
        };
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut sink = PortalOutputSink::new(portal, transport)
            .with_handshake_timeout(Duration::from_secs(60));
        let cancelled = || flag.load(Ordering::Relaxed);
        let error = sink.prepare_cancellable(&cancelled).unwrap_err();
        assert_eq!(error, DesktopOutputError::Cancelled);
        assert!(sink.session().is_none(), "session released");
        assert_eq!(sink.portal.inner.close_calls, 1);
    }

    /// Without cancellation, `prepare_cancellable` behaves exactly like
    /// `prepare`.
    #[test]
    fn prepare_cancellable_without_cancellation_is_prepare() {
        let (portal, transport, _) = happy_handshake();
        let mut sink = PortalOutputSink::new(portal, transport);
        let cancelled = || false;
        sink.prepare_cancellable(&cancelled).unwrap();
        assert_eq!(sink.state(), SessionState::Emulating);
    }

    // ── M6 cleanup: the real device type is queried ──

    /// A physical device (deltas in millimetres) is never chosen: the
    /// logical-pixel mapping is only claimed for virtual devices. If only a
    /// physical device appears, the handshake fails with the honest
    /// `CapabilityMissing` reason.
    #[test]
    fn physical_device_is_rejected_before_claiming_logical_pixels() {
        let portal = FakePortal::success();
        let device = DeviceId(7);
        let mut transport = FakeTransport::new();
        transport.events = VecDeque::from([
            TransportEvent::Connected,
            TransportEvent::SeatAdded {
                seat: crate::transport::SeatId(1),
            },
            TransportEvent::DeviceAdded {
                device,
                capabilities: BIND_CAPABILITY_BITS,
                device_type: DeviceType::Physical,
            },
        ]);
        let mut sink = PortalOutputSink::new(portal, transport)
            .with_handshake_timeout(Duration::from_millis(50));
        let error = sink.prepare().unwrap_err();
        assert!(
            matches!(&error, DesktopOutputError::CapabilityMissing(message) if message.contains("millimetres")),
            "{error:?}"
        );
        assert!(sink.session().is_none());
    }

    /// A physical device followed by a virtual device still succeeds: the
    /// adapter waits for a virtual device instead of grabbing the physical
    /// one.
    #[test]
    fn virtual_device_after_physical_is_chosen() {
        let portal = FakePortal::success();
        let physical = DeviceId(6);
        let virtual_device = DeviceId(7);
        let mut transport = FakeTransport::new();
        transport.events = VecDeque::from([
            TransportEvent::Connected,
            TransportEvent::SeatAdded {
                seat: crate::transport::SeatId(1),
            },
            TransportEvent::DeviceAdded {
                device: physical,
                capabilities: BIND_CAPABILITY_BITS,
                device_type: DeviceType::Physical,
            },
            TransportEvent::DeviceAdded {
                device: virtual_device,
                capabilities: BIND_CAPABILITY_BITS,
                device_type: DeviceType::Virtual,
            },
            TransportEvent::DeviceResumed {
                device: virtual_device,
            },
        ]);
        let mut sink = PortalOutputSink::new(portal, transport);
        sink.prepare().unwrap();
        assert_eq!(sink.state(), SessionState::Emulating);
        assert_eq!(sink.device, Some(virtual_device));
    }

    // ── M6 re-review R5: per-axis scroll stop on the wire ──

    /// The fixed probe pattern scrolls only y; the ScrollEnd must stop only
    /// y (regression for the review finding that X was stopped as well).
    #[test]
    fn scroll_end_stops_only_the_active_axes() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-120.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ScrollEnd).unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::ScrollStop {
                    device: d,
                    stop_x: false,
                    stop_y: true
                } if *d == device
            )),
            "expected stop_y-only ScrollStop: {log:?}"
        );
    }

    /// Two-axis scroll stops both axes.
    #[test]
    fn two_axis_scroll_end_stops_both_axes() {
        let (mut sink, device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(-10.0),
            dy: px(-120.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ScrollEnd).unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::ScrollStop {
                    device: d,
                    stop_x: true,
                    stop_y: true
                } if *d == device
            )),
            "expected two-axis ScrollStop: {log:?}"
        );
    }

    /// Partial send failure mid-scroll: only the axes whose deltas were
    /// actually sent are stopped on release.
    #[test]
    fn partial_scroll_send_stops_only_sent_axes() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        // x delta succeeds (recorded), y delta fails (not recorded).
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(-10.0),
            dy: px(0.0),
        })
        .unwrap();
        sink.transport.send_error = Some(DesktopOutputError::TransportDisconnected(
            "injected".to_string(),
        ));
        assert!(sink
            .submit(OutputEvent::ScrollDelta {
                dx: px(0.0),
                dy: px(-120.0),
            })
            .is_err());
        sink.transport.send_error = None;
        // Release stops only x.
        sink.release_all().unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::ScrollStop {
                    stop_x: true,
                    stop_y: false,
                    ..
                }
            )),
            "expected stop_x-only ScrollStop: {log:?}"
        );
        assert!(
            log.iter()
                .all(|call| !matches!(call, FakeWireCall::ScrollStop { stop_y: true, .. })),
            "y was never sent, must not be stopped: {log:?}"
        );
    }

    /// Forced release mid-scroll stops exactly the active axes and nothing
    /// else.
    #[test]
    fn forced_release_stops_active_axes() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-240.0),
        })
        .unwrap();
        sink.release_all().unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::ScrollStop {
                    stop_x: false,
                    stop_y: true,
                    ..
                }
            )),
            "expected stop_y-only ScrollStop on forced release: {log:?}"
        );
    }

    /// A zero-delta scroll lifecycle releases without any ScrollStop.
    #[test]
    fn zero_delta_scroll_releases_without_stop() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(0.0),
        })
        .unwrap();
        sink.release_all().unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter()
                .all(|call| !matches!(call, FakeWireCall::ScrollStop { .. })),
            "nothing was scrolled; no ScrollStop expected: {log:?}"
        );
    }

    /// M6 re-review R9: an explicit `begin → zero delta → end` interaction
    /// is a **purely local** lifecycle — no wire stop, no wire delta, and no
    /// frame — not the invalid `scroll_stop(false, false)` + frame the
    /// previous repair still sent on the explicit `ScrollEnd` path (it only
    /// avoided the call during forced cleanup). The local marker also resets
    /// the per-axis state for the next interaction.
    #[test]
    fn explicit_zero_axis_scroll_end_is_local_with_no_wire_stop_and_no_frame() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(0.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ScrollEnd).unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().all(|call| !matches!(
                call,
                FakeWireCall::ScrollDelta { .. }
                    | FakeWireCall::ScrollStop { .. }
                    | FakeWireCall::Frame { .. }
            )),
            "a zero-axis scroll lifecycle must not touch the wire: {log:?}"
        );
        // The lifecycle still closed cleanly and emission remains possible.
        assert!(sink.held.is_clean());
        assert_eq!(sink.state(), SessionState::Emulating);

        // A later nonzero lifecycle emits normally: the local ScrollEnd
        // reset the per-axis state, so the new y-only interaction stops only
        // y.
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(-120.0),
        })
        .unwrap();
        sink.submit(OutputEvent::ScrollEnd).unwrap();
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::ScrollStop {
                    stop_x: false,
                    stop_y: true,
                    ..
                }
            )),
            "the next interaction must stop only its own active axis: {log:?}"
        );
    }

    // ── M6 re-review R3: cancellation between pattern steps still cleans up ──

    /// When the pattern runner observes cancellation between steps, the
    /// ordered release still runs (exit 8 path at the desktop level).
    #[test]
    fn cancellation_between_steps_still_runs_ordered_release() {
        let (mut sink, _device) = prepared_sink();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        let cancelled = || true;
        let mut sleeper = |_: Duration| {};
        let mut progress = |_: &str| {};
        let mut driver = crate::desktop::EmitDriver {
            sleeper: &mut sleeper,
            progress: &mut progress,
            cancelled: &cancelled,
        };
        let capabilities = sink.capabilities();
        let result = crate::emit::run_pattern(&mut sink, capabilities, &mut driver);
        assert_eq!(result.unwrap_err(), DesktopOutputError::Cancelled);
        // The ordered release runs after the cancellation: button released,
        // disconnect, session closed.
        sink.release_all().unwrap();
        assert_eq!(sink.state(), SessionState::Stopped);
        assert_eq!(sink.portal.close_calls, 1);
        let log = sink.transport.log().to_vec();
        assert!(
            log.iter().any(|call| matches!(
                call,
                FakeWireCall::Button {
                    is_press: false,
                    ..
                }
            )),
            "held button released after cancellation: {log:?}"
        );
    }

    /// A test-only transport wrapper that flips a cancellation flag on the
    /// first handshake `wait_event`, so tests can prove the handshake loop
    /// observes cancellation *between* waits (after the portal steps
    /// completed). Delegates every other call to the inner fake.
    struct CancelOnHandshakeWait {
        inner: FakeTransport,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Transport for CancelOnHandshakeWait {
        fn connect(&mut self, fd: i32) -> Result<(), DesktopOutputError> {
            self.inner.connect(fd)
        }
        fn wait_event(&mut self, timeout: Duration) -> Result<TransportEvent, DesktopOutputError> {
            self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
            self.inner.wait_event(timeout)
        }
        fn pump(&mut self) -> Result<Vec<TransportEvent>, DesktopOutputError> {
            self.inner.pump()
        }
        fn bind_capabilities(
            &mut self,
            seat: SeatId,
            capabilities: u32,
        ) -> Result<(), DesktopOutputError> {
            self.inner.bind_capabilities(seat, capabilities)
        }
        fn start_emulating(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
            self.inner.start_emulating(device)
        }
        fn pointer_motion(
            &mut self,
            device: DeviceId,
            dx: f64,
            dy: f64,
        ) -> Result<(), DesktopOutputError> {
            self.inner.pointer_motion(device, dx, dy)
        }
        fn button(
            &mut self,
            device: DeviceId,
            button: u32,
            is_press: bool,
        ) -> Result<(), DesktopOutputError> {
            self.inner.button(device, button, is_press)
        }
        fn scroll_delta(
            &mut self,
            device: DeviceId,
            dx: f64,
            dy: f64,
        ) -> Result<(), DesktopOutputError> {
            self.inner.scroll_delta(device, dx, dy)
        }
        fn scroll_stop(
            &mut self,
            device: DeviceId,
            stop_x: bool,
            stop_y: bool,
        ) -> Result<(), DesktopOutputError> {
            self.inner.scroll_stop(device, stop_x, stop_y)
        }
        fn frame(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
            self.inner.frame(device)
        }
        fn disconnect(&mut self) -> Result<(), DesktopOutputError> {
            self.inner.disconnect()
        }
    }

    /// A test-only portal wrapper that flips a cancellation flag right after
    /// the authorization `start` succeeds, so tests can prove a cancellation
    /// observed after authorization releases the authorized session.
    /// Delegates every other call to the inner fake.
    struct CancelAfterAuthorize {
        inner: FakePortal,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Portal for CancelAfterAuthorize {
        fn create_session(&mut self) -> Result<PortalSession, DesktopOutputError> {
            self.inner.create_session()
        }
        fn select_devices(
            &mut self,
            session: &PortalSession,
            types: u32,
        ) -> Result<(), DesktopOutputError> {
            self.inner.select_devices(session, types)
        }
        fn start(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
            self.inner.start(session)?;
            self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        fn connect_to_eis(&mut self, session: &PortalSession) -> Result<EisFd, DesktopOutputError> {
            self.inner.connect_to_eis(session)
        }
        fn close_session(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
            self.inner.close_session(session)
        }
    }
}
