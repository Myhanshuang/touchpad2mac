#![forbid(unsafe_code)]
//! The reusable **prepared streaming output session** for M10 (M10_TASK.md
//! §5).
//!
//! M6 proved the portal/EIS/libei protocol path with a **fixed bounded
//! probe pattern** ([`crate::emit::run_pattern`]). M10's takeover needs a
//! *streaming* session instead: a reusable session that implements the typed
//! [`touchpad_core::OutputSink`] contract and exposes the negotiated
//! capabilities and readiness, so the takeover coordinator can prepare it
//! once (portal authorization → EIS connection → libei handshake), then
//! submit the resolved pointer/button/scroll events of the M7–M9 arbiter as
//! they are produced. The fixed M6 probe pattern is **not** the streaming
//! API and is never replayed by a takeover.
//!
//! # Boundary
//!
//! The zbus/libei/native types stay inside `touchpad-desktop`:
//!
//! * [`StreamingOutput`] is the session contract the takeover coordinator
//!   drives — [`prepare`](StreamingOutput::prepare) (cancellable, bounded
//!   exactly as M6), the [`OutputSink`] submission path (synchronous
//!   accepted/rejected semantics preserved), and the idempotent
//!   [`release_all`](OutputSink::release_all) that performs the explicit
//!   semantic releases, the transport disconnect, and the portal session
//!   close.
//! * [`StreamingOutputFactory`] builds sessions: production uses
//!   [`RealStreamingOutputFactory`] (RemoteDesktop portal + runtime-loaded
//!   libei sender); tests inject a fake factory and never connect to D-Bus,
//!   Wayland, the portal, or libei. Session **construction** is pure object
//!   allocation — the session-bus connection and the libei dlopen are
//!   deferred into [`StreamingOutput::prepare`] (after the device has
//!   opened; M10 review R6), so a missing/invalid device never triggers
//!   D-Bus/libei/output access.
//! * [`PortalStreamingOutput`] wraps the already-reviewed M6
//!   [`crate::sink::PortalOutputSink`] into the streaming contract.
//!
//! # Guarantees (inherited from the M6 sink, §17.3 of DESIGN_V2)
//!
//! * Preparation is cancellable and bounded (portal waits bounded, handshake
//!   ≤ 15 s, cancellation-aware).
//! * A server-side pause/removal/disconnect after the handshake is a
//!   **terminal output fault**: after the first rejected semantic event no
//!   later wire output is allowed; the structured interruption is preserved
//!   for the caller ([`StreamingOutput::take_server_interruption`]).
//! * `release_all` is idempotent and performs the explicit semantic
//!   releases, the transport disconnect (the compositor-side reset
//!   backstop), and the portal session close; failures are preserved
//!   structurally ([`StreamingOutput::take_cleanup_error`]).
//! * Only resolved pointer/button/scroll events are emitted — no virtual
//!   touchpad is constructed and no raw contacts/finger count are ever
//!   forwarded (the M6 adapter never binds the touch capability).

use std::time::Duration;

use touchpad_core::{
    DesktopAction, Monotonic, OutputError, OutputEvent, OutputFrameError, OutputSink,
};

use crate::capabilities::OutputCapabilities;
use crate::error::DesktopOutputError;
use crate::kde_actions::{
    KGlobalAccelTransport, KdeActionAdapter, KdeActionMap, KdeActionTransport,
};
use crate::portal::{EisFd, Portal, PortalSession};
use crate::sink::{PortalOutputSink, SessionState};
use crate::transport::{DeviceId, SeatId, Transport, TransportEvent};

/// A reusable, prepared streaming output session (M10_TASK.md §5).
///
/// Implementors are [`OutputSink`]s whose [`prepare`](Self::prepare) must be
/// called once before any submission: it performs the output preparation and
/// authorization (portal session, device selection, user authorization, EIS
/// connection, capability negotiation, device resume) and returns the
/// negotiated capabilities. After `prepare` the session is `Emulating` and
/// accepts resolved semantic events; a server interruption makes it
/// terminal-faulted (no later wire output).
pub trait StreamingOutput: OutputSink {
    /// Prepares and authorizes the streaming session (cancellable, bounded
    /// exactly as M6). Returns the negotiated capabilities; on failure the
    /// partially-prepared session is released internally and the primary
    /// failure (with any cleanup failure) is returned.
    fn prepare(
        &mut self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<OutputCapabilities, DesktopOutputError>;

    /// The negotiated capabilities (valid after a successful `prepare`).
    fn capabilities(&self) -> OutputCapabilities;

    /// The current session lifecycle state.
    fn state(&self) -> SessionState;

    /// The structured server-side interruption observed by the session (device
    /// pause/removal, seat removal, disconnect), if any — consumed by the
    /// caller so the failure keeps its structured category.
    fn take_server_interruption(&mut self) -> Option<DesktopOutputError>;

    /// The detailed cleanup failure preserved by the last release, if any —
    /// consumed by the caller that reports it.
    fn take_cleanup_error(&mut self) -> Option<DesktopOutputError>;
}

/// Builds streaming output sessions (M10_TASK.md §5: "Production code uses
/// the real streaming factory; tests inject a fake session/factory and never
/// connect to D-Bus, Wayland, portal, or libei").
pub trait StreamingOutputFactory {
    /// Creates a **session object** — pure, side-effect-free allocation
    /// (M10 review R6). No session-bus connection, libei loading, portal
    /// session, authorization, or emission happens here; the real factory
    /// defers **all** external work into [`StreamingOutput::prepare`], which
    /// the takeover coordinator runs only after the explicitly named device
    /// has opened and validated (M10_TASK.md §4), so a missing/invalid
    /// device never triggers D-Bus/libei/output access and keeps its
    /// device-error precedence.
    fn create(&mut self) -> Result<Box<dyn StreamingOutput>, DesktopOutputError>;
}

impl OutputSink for Box<dyn StreamingOutput> {
    /// `Box<dyn StreamingOutput>` is itself an [`OutputSink`] (the
    /// `StreamingOutput` supertrait), so the takeover pipeline can hold the
    /// session behind a trait object inside `ArbiterSink`.
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        (**self).submit(event)
    }

    fn submit_frame(&mut self, events: &[OutputEvent]) -> Result<(), OutputFrameError> {
        (**self).submit_frame(events)
    }

    fn submit_frame_at(
        &mut self,
        timestamp: Monotonic,
        events: &[OutputEvent],
    ) -> Result<(), OutputFrameError> {
        (**self).submit_frame_at(timestamp, events)
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        (**self).release_all()
    }
}

/// Composite M19 output session: pointer/button/scroll stay on the reviewed
/// portal+libei streaming session while discrete [`DesktopAction`] events are
/// routed synchronously through a KDE action adapter. The action side owns no
/// held state, so cleanup remains the inner streaming session's idempotent
/// release path.
pub struct KdeActionStreamingOutput<T: KdeActionTransport> {
    inner: Box<dyn StreamingOutput>,
    actions: KdeActionAdapter<T>,
    required_actions: Vec<DesktopAction>,
}

impl<T: KdeActionTransport> KdeActionStreamingOutput<T> {
    /// Creates the composite session. Construction is side-effect-free; KDE
    /// D-Bus preflight and portal/libei preparation both occur later in
    /// [`StreamingOutput::prepare`].
    #[must_use]
    pub fn new(
        inner: Box<dyn StreamingOutput>,
        actions: KdeActionAdapter<T>,
        required_actions: Vec<DesktopAction>,
    ) -> Self {
        Self {
            inner,
            actions,
            required_actions,
        }
    }
}

impl<T: KdeActionTransport> OutputSink for KdeActionStreamingOutput<T> {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        match event {
            OutputEvent::DesktopAction(action) => self.actions.trigger(action),
            other => self.inner.submit(other),
        }
    }

    fn submit_frame(&mut self, events: &[OutputEvent]) -> Result<(), OutputFrameError> {
        let mut index = 0;
        while index < events.len() {
            if let OutputEvent::DesktopAction(action) = &events[index] {
                if let Err(primary) = self.actions.trigger(*action) {
                    return Err(OutputFrameError {
                        failed_index: index,
                        accepted_prefix: index,
                        primary,
                    });
                }
                index += 1;
                continue;
            }

            let start = index;
            while index < events.len() && !matches!(events[index], OutputEvent::DesktopAction(_)) {
                index += 1;
            }
            if let Err(error) = self.inner.submit_frame(&events[start..index]) {
                return Err(OutputFrameError {
                    failed_index: start + error.failed_index,
                    accepted_prefix: start + error.accepted_prefix,
                    primary: error.primary,
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
        let mut index = 0;
        while index < events.len() {
            if let OutputEvent::DesktopAction(action) = &events[index] {
                if let Err(primary) = self.actions.trigger(*action) {
                    return Err(OutputFrameError {
                        failed_index: index,
                        accepted_prefix: index,
                        primary,
                    });
                }
                index += 1;
                continue;
            }

            let start = index;
            while index < events.len() && !matches!(events[index], OutputEvent::DesktopAction(_)) {
                index += 1;
            }
            if let Err(error) = self.inner.submit_frame_at(timestamp, &events[start..index]) {
                return Err(OutputFrameError {
                    failed_index: start + error.failed_index,
                    accepted_prefix: start + error.accepted_prefix,
                    primary: error.primary,
                });
            }
        }
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        self.inner.release_all()
    }
}

impl<T: KdeActionTransport> StreamingOutput for KdeActionStreamingOutput<T> {
    fn prepare(
        &mut self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<OutputCapabilities, DesktopOutputError> {
        if cancelled() {
            return Err(DesktopOutputError::Cancelled);
        }
        self.actions
            .preflight_actions(&self.required_actions)
            .map_err(|error| {
                DesktopOutputError::CapabilityMissing(format!(
                    "KDE desktop-action preflight failed: {error}"
                ))
            })?;
        if cancelled() {
            return Err(DesktopOutputError::Cancelled);
        }
        self.inner.prepare(cancelled)
    }

    fn capabilities(&self) -> OutputCapabilities {
        self.inner.capabilities()
    }

    fn state(&self) -> SessionState {
        self.inner.state()
    }

    fn take_server_interruption(&mut self) -> Option<DesktopOutputError> {
        self.inner.take_server_interruption()
    }

    fn take_cleanup_error(&mut self) -> Option<DesktopOutputError> {
        self.inner.take_cleanup_error()
    }
}

/// The real M10 streaming session: wraps the already-reviewed M6
/// [`PortalOutputSink`] (portal + libei sender) into the [`StreamingOutput`]
/// contract, delegating every operation.
#[derive(Debug)]
pub struct PortalStreamingOutput<P: Portal, T: Transport> {
    inner: PortalOutputSink<P, T>,
}

impl<P: Portal, T: Transport> PortalStreamingOutput<P, T> {
    /// Creates a disconnected session over a portal + transport pair.
    #[must_use]
    pub fn new(portal: P, transport: T) -> Self {
        Self {
            inner: PortalOutputSink::new(portal, transport),
        }
    }

    /// Overrides the EIS handshake deadline (tests use a short deadline).
    #[must_use]
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.with_handshake_timeout(timeout);
        self
    }
}

impl<P: Portal, T: Transport> OutputSink for PortalStreamingOutput<P, T> {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        self.inner.submit(event)
    }

    fn submit_frame(&mut self, events: &[OutputEvent]) -> Result<(), OutputFrameError> {
        self.inner.submit_frame(events)
    }

    fn submit_frame_at(
        &mut self,
        timestamp: Monotonic,
        events: &[OutputEvent],
    ) -> Result<(), OutputFrameError> {
        self.inner.submit_frame_at(timestamp, events)
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        self.inner.release_all()
    }
}

impl<P: Portal, T: Transport> StreamingOutput for PortalStreamingOutput<P, T> {
    fn prepare(
        &mut self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<OutputCapabilities, DesktopOutputError> {
        self.inner.prepare_cancellable(cancelled)
    }

    fn capabilities(&self) -> OutputCapabilities {
        self.inner.capabilities()
    }

    fn state(&self) -> SessionState {
        self.inner.state()
    }

    fn take_server_interruption(&mut self) -> Option<DesktopOutputError> {
        self.inner.take_server_interruption()
    }

    fn take_cleanup_error(&mut self) -> Option<DesktopOutputError> {
        self.inner.take_cleanup_error()
    }
}

/// A [`Portal`] whose external construction happens **lazily**, on the first
/// portal call — i.e. inside [`StreamingOutput::prepare`], which the M10
/// takeover coordinator runs only **after** the device has opened and
/// validated (M10_TASK.md §4 preparation order; M10 review R6).
///
/// `F` is the provider of the inner portal; the real factory supplies one
/// that connects to the D-Bus session bus ([`crate::portal_zbus::ZbusPortal`]
/// construction), while tests inject a recording provider so the observable
/// factory/preparation timeline — object allocation at `create`, external
/// work at `prepare` — is provable without a real session bus. The provider
/// runs at most once; a failed construction is preserved and returned again
/// (no retry, matching the one-shot `prepare` contract).
pub struct LazyPortal<P: Portal, F: Fn() -> Result<P, DesktopOutputError>> {
    /// The external-work provider, invoked at most once.
    provider: F,
    /// The constructed inner portal (or its construction failure).
    cell: std::sync::OnceLock<Result<P, DesktopOutputError>>,
}

impl<P: Portal, F: Fn() -> Result<P, DesktopOutputError>> LazyPortal<P, F> {
    /// Creates a lazy portal over a provider that constructs the real portal
    /// on first use (external work deferred to `prepare`).
    #[must_use]
    pub fn new(provider: F) -> Self {
        Self {
            provider,
            cell: std::sync::OnceLock::new(),
        }
    }

    /// The inner portal, constructing it on first use.
    fn portal_mut(&mut self) -> Result<&mut P, DesktopOutputError> {
        if self.cell.get().is_none() {
            let _ = self.cell.set((self.provider)());
        }
        self.cell
            .get_mut()
            .expect("initialized above")
            .as_mut()
            .map_err(|error| (*error).clone())
    }
}

impl<P: Portal, F: Fn() -> Result<P, DesktopOutputError>> std::fmt::Debug for LazyPortal<P, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyPortal")
            .field("initialized", &self.cell.get().is_some())
            .finish_non_exhaustive()
    }
}

impl<P: Portal, F: Fn() -> Result<P, DesktopOutputError>> Portal for LazyPortal<P, F> {
    fn create_session(&mut self) -> Result<PortalSession, DesktopOutputError> {
        self.portal_mut()?.create_session()
    }

    fn select_devices(
        &mut self,
        session: &PortalSession,
        types: u32,
    ) -> Result<(), DesktopOutputError> {
        self.portal_mut()?.select_devices(session, types)
    }

    fn start(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
        self.portal_mut()?.start(session)
    }

    fn connect_to_eis(&mut self, session: &PortalSession) -> Result<EisFd, DesktopOutputError> {
        self.portal_mut()?.connect_to_eis(session)
    }

    fn close_session(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
        self.portal_mut()?.close_session(session)
    }
}

/// A [`Transport`] whose external construction happens **lazily**, inside
/// [`Transport::connect`] — the first preparation step that needs it, which
/// the M10 takeover coordinator runs only **after** the device has opened
/// and validated (M10_TASK.md §4; M10 review R6).
///
/// `F` is the provider of the inner transport; the real factory supplies one
/// that loads the runtime libei library, while tests inject a recording
/// provider so the observable factory/preparation timeline is provable
/// without a real library. The provider runs at most once, inside
/// `connect`; a `disconnect` before any `connect` is an idempotent no-op
/// (the real transport's disconnect is likewise safe when never connected).
pub struct LazyTransport<T: Transport, F: Fn() -> Result<T, DesktopOutputError>> {
    /// The external-work provider, invoked at most once (on first connect).
    provider: F,
    /// The constructed inner transport, once connected.
    inner: Option<T>,
}

impl<T: Transport, F: Fn() -> Result<T, DesktopOutputError>> LazyTransport<T, F> {
    /// Creates a lazy transport over a provider that constructs the real
    /// transport on first connect (external work deferred to `prepare`).
    #[must_use]
    pub fn new(provider: F) -> Self {
        Self {
            provider,
            inner: None,
        }
    }

    /// The inner transport, constructing it on first use.
    fn transport_mut(&mut self) -> Result<&mut T, DesktopOutputError> {
        if self.inner.is_none() {
            self.inner = Some((self.provider)()?);
        }
        Ok(self.inner.as_mut().expect("just initialized"))
    }
}

impl<T: Transport, F: Fn() -> Result<T, DesktopOutputError>> std::fmt::Debug
    for LazyTransport<T, F>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyTransport")
            .field("initialized", &self.inner.is_some())
            .finish_non_exhaustive()
    }
}

impl<T: Transport, F: Fn() -> Result<T, DesktopOutputError>> Transport for LazyTransport<T, F> {
    fn connect(&mut self, fd: i32) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.connect(fd)
    }

    fn wait_event(&mut self, timeout: Duration) -> Result<TransportEvent, DesktopOutputError> {
        self.transport_mut()?.wait_event(timeout)
    }

    fn pump(&mut self) -> Result<Vec<TransportEvent>, DesktopOutputError> {
        self.transport_mut()?.pump()
    }

    fn bind_capabilities(
        &mut self,
        seat: SeatId,
        capabilities: u32,
    ) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.bind_capabilities(seat, capabilities)
    }

    fn start_emulating(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.start_emulating(device)
    }

    fn pointer_motion(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.pointer_motion(device, dx, dy)
    }

    fn button(
        &mut self,
        device: DeviceId,
        button: u32,
        is_press: bool,
    ) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.button(device, button, is_press)
    }

    fn scroll_delta(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.scroll_delta(device, dx, dy)
    }

    fn scroll_stop(
        &mut self,
        device: DeviceId,
        stop_x: bool,
        stop_y: bool,
    ) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.scroll_stop(device, stop_x, stop_y)
    }

    fn frame(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        self.transport_mut()?.frame(device)
    }

    fn disconnect(&mut self) -> Result<(), DesktopOutputError> {
        match self.inner.as_mut() {
            Some(inner) => inner.disconnect(),
            // Never connected: the disconnect is an idempotent no-op (the
            // real transport's disconnect is likewise safe unconnected).
            None => Ok(()),
        }
    }
}

/// The real streaming factory: RemoteDesktop portal (zbus) + runtime-loaded
/// libei sender transport. Linux-only (the libei native transport is a Linux
/// surface).
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct RealStreamingOutputFactory;

#[cfg(target_os = "linux")]
impl StreamingOutputFactory for RealStreamingOutputFactory {
    fn create(&mut self) -> Result<Box<dyn StreamingOutput>, DesktopOutputError> {
        // Side-effect-free session **object allocation** (M10 review R6):
        // the session-bus connection and the libei dlopen are deferred into
        // [`StreamingOutput::prepare`], which the takeover coordinator runs
        // only after the device has opened and validated — so a
        // missing/invalid device never triggers D-Bus/libei/output access
        // and the device-open failure keeps its exit-code precedence.
        let portal = LazyPortal::new(crate::portal_zbus::ZbusPortal::connect);
        let transport = LazyTransport::new(|| {
            let libei = crate::ffi::Libei::load()?;
            Ok(crate::native_transport::NativeTransport::new(libei))
        });
        Ok(Box::new(PortalStreamingOutput::new(portal, transport)))
    }
}

/// The honest non-Linux fallback factory: creating a session refuses with a
/// structured unsupported-platform error (the takeover command cannot run
/// without the real Linux input/output path anyway).
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Default)]
pub struct RealStreamingOutputFactory;

#[cfg(not(target_os = "linux"))]
impl StreamingOutputFactory for RealStreamingOutputFactory {
    fn create(&mut self) -> Result<Box<dyn StreamingOutput>, DesktopOutputError> {
        Err(DesktopOutputError::UnsupportedPlatform(
            "the libei output backend is not built for this platform".to_string(),
        ))
    }
}

/// M19 real KDE factory: wraps the standard portal/libei streaming session
/// with a KGlobalAccel desktop-action channel. Construction remains pure;
/// both D-Bus action preflight and portal/libei authorization are deferred to
/// the returned session's `prepare` method.
#[derive(Debug, Clone)]
pub struct RealKdeStreamingOutputFactory {
    required_actions: Vec<DesktopAction>,
}

impl RealKdeStreamingOutputFactory {
    /// Creates a factory for the de-duplicated semantic desktop actions the
    /// current M19 settings may emit.
    #[must_use]
    pub fn new(required_actions: Vec<DesktopAction>) -> Self {
        Self { required_actions }
    }
}

impl StreamingOutputFactory for RealKdeStreamingOutputFactory {
    fn create(&mut self) -> Result<Box<dyn StreamingOutput>, DesktopOutputError> {
        let mut base = RealStreamingOutputFactory;
        let inner = base.create()?;
        let actions = KdeActionAdapter::new(KdeActionMap::default(), KGlobalAccelTransport::new());
        Ok(Box::new(KdeActionStreamingOutput::new(
            inner,
            actions,
            self.required_actions.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fake::{FakePortal, FakeStreamingOutput, FakeStreamingState, FakeTransport};
    use crate::transport::{DeviceId, TransportEvent};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use touchpad_core::{LogicalPixels, MouseButton};

    fn px(x: f32) -> LogicalPixels {
        LogicalPixels::try_new(x).unwrap()
    }

    #[derive(Clone, Default)]
    struct FakeActionTransport {
        preflight: Rc<RefCell<Vec<String>>>,
        invoked: Rc<RefCell<Vec<String>>>,
        fail_preflight: bool,
    }

    impl KdeActionTransport for FakeActionTransport {
        fn preflight(&mut self, bindings: &[&str]) -> Result<(), OutputError> {
            if self.fail_preflight {
                return Err(OutputError::Unavailable(
                    "fake KDE capability missing".to_string(),
                ));
            }
            self.preflight
                .borrow_mut()
                .extend(bindings.iter().map(|binding| (*binding).to_string()));
            Ok(())
        }

        fn invoke(&mut self, binding: &str) -> Result<(), OutputError> {
            self.invoked.borrow_mut().push(binding.to_string());
            Ok(())
        }
    }

    #[test]
    fn kde_composite_routes_actions_away_from_libei_and_preserves_cleanup() {
        let state = Rc::new(RefCell::new(FakeStreamingState::happy()));
        let inner: Box<dyn StreamingOutput> = Box::new(FakeStreamingOutput::new(Rc::clone(&state)));
        let transport = FakeActionTransport::default();
        let preflight = Rc::clone(&transport.preflight);
        let invoked = Rc::clone(&transport.invoked);
        let adapter = KdeActionAdapter::new(KdeActionMap::default(), transport);
        let mut session =
            KdeActionStreamingOutput::new(inner, adapter, vec![DesktopAction::PreviousWorkspace]);

        session.prepare(&|| false).unwrap();
        assert_eq!(&*preflight.borrow(), &["workspace-previous"]);
        session
            .submit(OutputEvent::DesktopAction(DesktopAction::PreviousWorkspace))
            .unwrap();
        assert_eq!(&*invoked.borrow(), &["workspace-previous"]);
        assert!(state.borrow().submitted.is_empty());

        session
            .submit(OutputEvent::PointerMove {
                dx: px(3.0),
                dy: px(-2.0),
            })
            .unwrap();
        assert_eq!(state.borrow().submitted.len(), 1);
        session.release_all().unwrap();
        assert_eq!(state.borrow().release_calls, 1);
    }

    #[test]
    fn kde_preflight_failure_happens_before_inner_portal_prepare() {
        let state = Rc::new(RefCell::new(FakeStreamingState::happy()));
        let inner: Box<dyn StreamingOutput> = Box::new(FakeStreamingOutput::new(Rc::clone(&state)));
        let transport = FakeActionTransport {
            fail_preflight: true,
            ..FakeActionTransport::default()
        };
        let adapter = KdeActionAdapter::new(KdeActionMap::default(), transport);
        let mut session =
            KdeActionStreamingOutput::new(inner, adapter, vec![DesktopAction::OpenOverview]);

        assert!(matches!(
            session.prepare(&|| false),
            Err(DesktopOutputError::CapabilityMissing(_))
        ));
        assert_eq!(state.borrow().prepare_calls, 0);
    }

    /// The real streaming session delegates `prepare`, `submit`,
    /// `release_all`, and the capability/interruption accessors to the
    /// underlying M6 portal sink — proven through the fake portal/transport
    /// (no real portal, D-Bus, or libei is ever constructed).
    #[test]
    fn portal_streaming_output_delegates_to_the_m6_sink() {
        let portal = FakePortal::success();
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut session = PortalStreamingOutput::new(portal, transport);

        // Disconnected before prepare; submission is rejected.
        assert_eq!(session.state(), SessionState::Disconnected);
        assert!(session
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .is_err());

        // Prepare reaches Emulating and exposes the negotiated capabilities.
        let caps = session.prepare(&|| false).expect("prepare succeeds");
        assert!(caps.supports(crate::capabilities::Capability::RelativePointer));
        assert_eq!(session.state(), SessionState::Emulating);

        // Streaming submission of resolved events works in order.
        session
            .submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        session
            .submit(OutputEvent::ButtonUp(MouseButton::Left))
            .unwrap();

        // release_all is idempotent and returns to Stopped.
        session.release_all().unwrap();
        assert_eq!(session.state(), SessionState::Stopped);
        session.release_all().unwrap();
        assert!(session.take_cleanup_error().is_none());
        assert!(session.take_server_interruption().is_none());
    }

    #[test]
    fn portal_streaming_output_prepare_failure_is_released() {
        let mut portal = FakePortal::success();
        portal.start_behavior = Some(DesktopOutputError::AuthorizationCancelled);
        let transport = FakeTransport::happy_handshake(DeviceId(7));
        let mut session = PortalStreamingOutput::new(portal, transport);
        let error = session.prepare(&|| false).unwrap_err();
        assert_eq!(error, DesktopOutputError::AuthorizationCancelled);
        // The failed prepare must not leave a live session.
        assert!(matches!(
            session.state(),
            SessionState::Stopped | SessionState::Fatal
        ));
    }

    /// M10 review R3: the **real session lifecycle clears the structured
    /// server interruption during `release_all`** (`PortalOutputSink`'
    /// `release_all_detailed` resets its interruption), so the takeover
    /// coordinator must capture `take_server_interruption` BEFORE the release
    /// or a real DevicePaused/DeviceRemoved/SeatRemoved/Disconnect primary is
    /// lost and flattened into a generic semantic-output failure. Driven
    /// through `PortalStreamingOutput<FakePortal, FakeTransport>` — the
    /// actual release behavior — proving (1) the interruption is retrievable
    /// before the release, and (2) the release clears it, which is exactly
    /// why the capture order matters.
    #[test]
    fn server_interruption_must_be_captured_before_release_all() {
        let portal = FakePortal::success();
        let mut transport = FakeTransport::happy_handshake(DeviceId(7));
        // The server pauses the active device after the handshake: the next
        // submit's pump observes it and stores a structured interruption.
        transport.events.push_back(TransportEvent::DevicePaused {
            device: DeviceId(7),
        });
        let mut session = PortalStreamingOutput::new(portal, transport);
        session.prepare(&|| false).expect("prepare succeeds");
        // The pump around this submit observes the pause and transitions the
        // session out of Emulating with the structured interruption.
        let error = session
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .unwrap_err();
        assert!(matches!(error, OutputError::Io(_)), "{error}");
        // The structured category is retrievable BEFORE the release...
        let interruption = session.take_server_interruption();
        assert!(
            matches!(interruption, Some(DesktopOutputError::DevicePaused(_))),
            "{interruption:?}"
        );
        // ... but the actual release behavior CLEARS it, so a coordinator
        // reading it after `release_all` would lose the category.
        session.release_all().unwrap();
        assert!(session.take_server_interruption().is_none());
    }

    /// M10 review R6: the observable factory/preparation timeline — the real
    /// factory's `create` must perform **zero external work** (object
    /// allocation only); the session-bus connection / libei loading happen
    /// inside `prepare`, after the device has opened. The lazy portal and
    /// transport providers record their invocations: `create` (allocation)
    /// invokes neither, `prepare` (the external work) invokes both exactly
    /// once, and the session becomes fully usable.
    #[test]
    fn lazy_factory_timeline_allocation_at_create_external_work_at_prepare() {
        let portal_calls = Rc::new(Cell::new(0usize));
        let transport_calls = Rc::new(Cell::new(0usize));
        let pc = Rc::clone(&portal_calls);
        let tc = Rc::clone(&transport_calls);
        let portal: LazyPortal<FakePortal, _> = LazyPortal::new(move || {
            pc.set(pc.get() + 1);
            Ok(FakePortal::success())
        });
        let transport: LazyTransport<FakeTransport, _> = LazyTransport::new(move || {
            tc.set(tc.get() + 1);
            Ok(FakeTransport::happy_handshake(DeviceId(7)))
        });
        let mut session = PortalStreamingOutput::new(portal, transport);

        // Object allocation at "create": no external work has happened, the
        // session is disconnected with no negotiated capabilities, and
        // submission is rejected.
        assert_eq!(portal_calls.get(), 0, "create must not touch the portal");
        assert_eq!(
            transport_calls.get(),
            0,
            "create must not load/connect the transport"
        );
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(session.capabilities(), OutputCapabilities::NONE);
        assert!(session
            .submit(OutputEvent::PointerMove {
                dx: px(1.0),
                dy: px(0.0),
            })
            .is_err());

        // `prepare` performs the external work, exactly once each, and the
        // session becomes emulating with the negotiated capabilities.
        let caps = session.prepare(&|| false).expect("prepare succeeds");
        assert_eq!(portal_calls.get(), 1, "prepare must build the portal once");
        assert_eq!(
            transport_calls.get(),
            1,
            "prepare must build the transport once"
        );
        assert_eq!(session.state(), SessionState::Emulating);
        assert!(caps.supports(crate::capabilities::Capability::RelativePointer));

        // The lazily-built session is fully usable.
        session
            .submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        session.release_all().unwrap();
        assert_eq!(session.state(), SessionState::Stopped);
    }
}
