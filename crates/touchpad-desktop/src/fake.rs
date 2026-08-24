#![forbid(unsafe_code)]
//! Deterministic fake portal/transport seams (M6 required outcome 5).
//!
//! Every automated test drives the real session logic
//! ([`crate::sink::PortalOutputSink`], the emit pattern runner, the probe
//! formatter) through these fakes — never through the real zbus portal or
//! the libei transport. The fakes record every wire call so tests can prove
//! ordering, capability negotiation, backpressure/partial failure,
//! disconnect, and repeated shutdown/release behavior without a Wayland
//! desktop, a session bus, a portal, or libei.
//!
//! **No fake ever emits real desktop input**: the fakes are pure in-memory
//! records.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use touchpad_core::{OutputError, OutputEvent, OutputSink};

use crate::capabilities::OutputCapabilities;
use crate::error::DesktopOutputError;
use crate::portal::{EisFd, Portal, PortalSession};
use crate::sink::SessionState;
use crate::transport::{DeviceId, DeviceType, SeatId, Transport, TransportEvent};

/// A recorded wire call of the fake transport.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeWireCall {
    /// `Transport::connect` with the given fd.
    Connect(i32),
    /// `Transport::bind_capabilities` — the seat and the raw capability
    /// bitmask.
    BindCapabilities {
        /// The seat being bound.
        seat: SeatId,
        /// The raw libei capability bits.
        capabilities: u32,
    },
    /// `Transport::start_emulating` — the device.
    StartEmulating {
        /// The device.
        device: DeviceId,
    },
    /// `Transport::pointer_motion` — device and relative delta.
    PointerMotion {
        /// The device.
        device: DeviceId,
        /// x delta in logical pixels.
        dx: f64,
        /// y delta in logical pixels.
        dy: f64,
    },
    /// `Transport::button` — device, Linux input code, press/release.
    Button {
        /// The device.
        device: DeviceId,
        /// The Linux input event code (`BTN_LEFT` 0x110, …).
        button: u32,
        /// Whether this is a press.
        is_press: bool,
    },
    /// `Transport::scroll_delta` — device and pixel delta.
    ScrollDelta {
        /// The device.
        device: DeviceId,
        /// x delta in logical pixels.
        dx: f64,
        /// y delta in logical pixels.
        dy: f64,
    },
    /// `Transport::scroll_stop` — device and the axes to stop.
    ScrollStop {
        /// The device.
        device: DeviceId,
        /// Stop the x axis.
        stop_x: bool,
        /// Stop the y axis.
        stop_y: bool,
    },
    /// `Transport::frame` — the device whose frame is closed.
    Frame {
        /// The device.
        device: DeviceId,
    },
    /// `Transport::disconnect`.
    Disconnect,
}

/// A fake [`Transport`] whose server events are a scripted queue and whose
/// emission calls are recorded (and can be injected with failures).
#[derive(Debug, Clone)]
pub struct FakeTransport {
    /// Scripted server events consumed by [`wait_event`](Transport::wait_event).
    pub events: VecDeque<TransportEvent>,
    /// When set, every emission call fails with this error (partial-send
    /// fault injection).
    pub send_error: Option<DesktopOutputError>,
    /// When set, `connect` fails with this error (preparation-stage fault
    /// injection, M6 re-review R4).
    pub connect_error: Option<DesktopOutputError>,
    /// When set, `disconnect` fails with this error.
    pub disconnect_error: Option<DesktopOutputError>,
    /// Every wire call, in order.
    pub log: Vec<FakeWireCall>,
    /// The device the emission calls are routed through.
    pub device: Option<DeviceId>,
    connected: bool,
}

impl FakeTransport {
    /// An empty fake transport.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            send_error: None,
            connect_error: None,
            disconnect_error: None,
            log: Vec::new(),
            device: None,
            connected: false,
        }
    }

    /// A happy-path handshake: connect, seat added, device added (with the
    /// full pointer/button/scroll capability set on a **virtual** device),
    /// device resumed.
    #[must_use]
    pub fn happy_handshake(device: DeviceId) -> Self {
        Self::happy_handshake_with_caps(device, crate::sink::BIND_CAPABILITY_BITS)
    }

    /// A happy-path handshake where the added device exposes the given raw
    /// libei capability bits on a **virtual** device (logical pixels).
    #[must_use]
    pub fn happy_handshake_with_caps(device: DeviceId, capabilities: u32) -> Self {
        let mut transport = Self::new();
        transport.events = VecDeque::from([
            TransportEvent::Connected,
            TransportEvent::SeatAdded { seat: SeatId(1) },
            TransportEvent::DeviceAdded {
                device,
                capabilities,
                device_type: DeviceType::Virtual,
            },
            TransportEvent::DeviceResumed { device },
        ]);
        transport.device = Some(device);
        transport
    }

    /// The recorded wire calls.
    #[must_use]
    pub fn log(&self) -> &[FakeWireCall] {
        &self.log
    }
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for FakeTransport {
    fn connect(&mut self, fd: i32) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::Connect(fd));
        if let Some(error) = &self.connect_error {
            return Err(error.clone());
        }
        self.connected = true;
        Ok(())
    }

    fn wait_event(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<TransportEvent, DesktopOutputError> {
        Ok(self.events.pop_front().unwrap_or(TransportEvent::Timeout))
    }

    fn pump(&mut self) -> Result<Vec<TransportEvent>, DesktopOutputError> {
        let mut events = Vec::new();
        loop {
            match self.events.pop_front() {
                None | Some(TransportEvent::Timeout) => break,
                // Disconnected is terminal: report it and stop (a repeated
                // wait would keep returning it).
                Some(TransportEvent::Disconnected) => {
                    events.push(TransportEvent::Disconnected);
                    break;
                }
                Some(event) => events.push(event),
            }
        }
        Ok(events)
    }

    fn bind_capabilities(
        &mut self,
        seat: SeatId,
        capabilities: u32,
    ) -> Result<(), DesktopOutputError> {
        self.log
            .push(FakeWireCall::BindCapabilities { seat, capabilities });
        Ok(())
    }

    fn start_emulating(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::StartEmulating { device });
        Ok(())
    }

    fn pointer_motion(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        self.log
            .push(FakeWireCall::PointerMotion { device, dx, dy });
        self.maybe_fail()
    }

    fn button(
        &mut self,
        device: DeviceId,
        button: u32,
        is_press: bool,
    ) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::Button {
            device,
            button,
            is_press,
        });
        self.maybe_fail()
    }

    fn scroll_delta(
        &mut self,
        device: DeviceId,
        dx: f64,
        dy: f64,
    ) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::ScrollDelta { device, dx, dy });
        self.maybe_fail()
    }

    fn scroll_stop(
        &mut self,
        device: DeviceId,
        stop_x: bool,
        stop_y: bool,
    ) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::ScrollStop {
            device,
            stop_x,
            stop_y,
        });
        self.maybe_fail()
    }

    fn frame(&mut self, device: DeviceId) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::Frame { device });
        self.maybe_fail()
    }

    fn disconnect(&mut self) -> Result<(), DesktopOutputError> {
        self.log.push(FakeWireCall::Disconnect);
        self.connected = false;
        self.device = None;
        if let Some(error) = &self.disconnect_error {
            return Err(error.clone());
        }
        Ok(())
    }
}

impl FakeTransport {
    fn maybe_fail(&self) -> Result<(), DesktopOutputError> {
        if let Some(error) = &self.send_error {
            Err(error.clone())
        } else {
            Ok(())
        }
    }
}

/// Which fake portal step should fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakePortalStep {
    /// `create_session`.
    CreateSession,
    /// `select_devices`.
    SelectDevices,
    /// `start` (authorization).
    Start,
    /// `connect_to_eis`.
    ConnectToEis,
    /// `close_session`.
    CloseSession,
}

/// A fake [`Portal`] recording every call; any step can be injected with a
/// failure.
#[derive(Debug, Clone)]
pub struct FakePortal {
    /// When set, the named step fails with this error.
    pub fail_step: Option<(FakePortalStep, DesktopOutputError)>,
    /// When set, `close_session` fails with this error **independently** of
    /// `fail_step` — so a preparation-stage failure and a cleanup failure can
    /// be injected at the same time (M6 re-review R4).
    pub close_error: Option<DesktopOutputError>,
    /// The device types passed to `select_devices` (for assertions).
    pub selected_types: Vec<u32>,
    /// Number of `close_session` calls (release assertions).
    pub close_calls: usize,
    /// Number of sessions created.
    pub sessions_created: usize,
    /// The EIS fd returned by `connect_to_eis`.
    pub eis_fd: i32,
    /// When set, `start` behaves as if the user cancelled/refused.
    pub start_behavior: Option<DesktopOutputError>,
}

impl FakePortal {
    /// A portal where every step succeeds.
    #[must_use]
    pub fn success() -> Self {
        Self {
            fail_step: None,
            close_error: None,
            selected_types: Vec::new(),
            close_calls: 0,
            sessions_created: 0,
            eis_fd: 42,
            start_behavior: None,
        }
    }

    fn fail(&self, step: FakePortalStep) -> Result<(), DesktopOutputError> {
        if let Some((failed, error)) = &self.fail_step {
            if *failed == step {
                return Err(error.clone());
            }
        }
        Ok(())
    }
}

impl Default for FakePortal {
    fn default() -> Self {
        Self::success()
    }
}

impl Portal for FakePortal {
    fn create_session(&mut self) -> Result<PortalSession, DesktopOutputError> {
        self.fail(FakePortalStep::CreateSession)?;
        self.sessions_created += 1;
        Ok(PortalSession(format!("/session/{}", self.sessions_created)))
    }

    fn select_devices(
        &mut self,
        _session: &PortalSession,
        types: u32,
    ) -> Result<(), DesktopOutputError> {
        self.fail(FakePortalStep::SelectDevices)?;
        self.selected_types.push(types);
        Ok(())
    }

    fn start(&mut self, _session: &PortalSession) -> Result<(), DesktopOutputError> {
        self.fail(FakePortalStep::Start)?;
        if let Some(error) = &self.start_behavior {
            return Err(error.clone());
        }
        Ok(())
    }

    fn connect_to_eis(&mut self, _session: &PortalSession) -> Result<EisFd, DesktopOutputError> {
        self.fail(FakePortalStep::ConnectToEis)?;
        Ok(EisFd(self.eis_fd))
    }

    fn close_session(&mut self, _session: &PortalSession) -> Result<(), DesktopOutputError> {
        if let Some(error) = &self.close_error {
            return Err(error.clone());
        }
        self.fail(FakePortalStep::CloseSession)?;
        self.close_calls += 1;
        Ok(())
    }
}

/// A fake [`crate::desktop::DesktopOutput`] used by the CLI tests: a canned
/// probe report and a canned emit outcome/error, plus call recording.
#[derive(Debug, Clone)]
pub struct FakeDesktopOutput {
    /// The report returned by `probe`.
    pub probe_report: crate::probe::ProbeReport,
    /// The result returned by `emit_pattern`.
    pub emit_result: Result<crate::emit::EmitOutcome, DesktopOutputError>,
    /// Whether `emit_pattern` was called.
    pub emit_called: bool,
}

impl FakeDesktopOutput {
    /// A fake whose probe reports a fully available environment and whose
    /// emit succeeds.
    #[must_use]
    pub fn available() -> Self {
        Self {
            probe_report: crate::probe::ProbeReport::available_for_tests(),
            emit_result: Ok(crate::emit::EmitOutcome::default()),
            emit_called: false,
        }
    }
}

impl crate::desktop::DesktopOutput for FakeDesktopOutput {
    fn probe(&self) -> crate::probe::ProbeReport {
        self.probe_report.clone()
    }

    fn emit_pattern(
        &mut self,
        _driver: &mut crate::desktop::EmitDriver<'_>,
    ) -> Result<crate::emit::EmitOutcome, DesktopOutputError> {
        self.emit_called = true;
        self.emit_result.clone()
    }
}

/// A fake [`crate::probe::ProbeSource`] with a canned report.
#[derive(Debug, Clone)]
pub struct FakeProbeSource {
    /// The canned report.
    pub report: crate::probe::ProbeReport,
}

impl crate::probe::ProbeSource for FakeProbeSource {
    fn probe(&self) -> crate::probe::ProbeReport {
        self.report.clone()
    }
}

// ---------------------------------------------------------------------------
// M10: the fake streaming output session and factory (M10_TASK.md §5: "tests
// inject a fake session/factory and never connect to D-Bus, Wayland, portal,
// or libei").
// ---------------------------------------------------------------------------

/// Mutable, test-observable state of a fake streaming session (M10).
///
/// The session object itself is moved into the takeover pipeline (as the
/// decoder sink's output), so all observations live in this shared cell the
/// test keeps a handle to.
#[derive(Debug)]
pub struct FakeStreamingState {
    /// How many times `prepare` was called.
    pub prepare_calls: usize,
    /// The result `prepare` returns (negotiated capabilities or an error).
    pub prepare_result: Result<OutputCapabilities, DesktopOutputError>,
    /// The negotiated capabilities exposed by `capabilities()`.
    pub capabilities: OutputCapabilities,
    /// The lifecycle state exposed by `state()`.
    pub state: SessionState,
    /// Every event submitted (in order) — including events a scripted
    /// rejection refuses, so tests can prove "no later wire output" after a
    /// fault.
    pub submitted: Vec<OutputEvent>,
    /// How many times the wrapped `release_all` was called.
    pub release_calls: usize,
    /// The result the wrapped `release_all` returns.
    pub release_result: Result<(), DesktopOutputError>,
    /// The simulated server interruption returned by
    /// `take_server_interruption`.
    pub interruption: Option<DesktopOutputError>,
    /// The cleanup error returned by `take_cleanup_error`.
    pub cleanup_error: Option<DesktopOutputError>,
    /// Scripted per-submit outcomes; consumed in order; exhausted → `Ok`.
    pub submit_script: VecDeque<Result<(), DesktopOutputError>>,
    /// Shared timeline for ordering assertions (each operation pushes a
    /// marker).
    pub timeline: Option<Rc<RefCell<Vec<String>>>>,
}

impl Default for FakeStreamingState {
    fn default() -> Self {
        Self {
            prepare_calls: 0,
            prepare_result: Err(DesktopOutputError::Internal(
                "fake session was never scripted to prepare successfully".to_string(),
            )),
            capabilities: OutputCapabilities::NONE,
            state: SessionState::Disconnected,
            submitted: Vec::new(),
            release_calls: 0,
            release_result: Ok(()),
            interruption: None,
            cleanup_error: None,
            submit_script: VecDeque::new(),
            timeline: None,
        }
    }
}

impl FakeStreamingState {
    /// A happy session: prepare succeeds with the full M6 capability set,
    /// every submit succeeds, release succeeds.
    #[must_use]
    pub fn happy() -> Self {
        Self {
            prepare_result: Ok(OutputCapabilities::from_device_capability_bits(
                crate::sink::BIND_CAPABILITY_BITS,
            )),
            capabilities: OutputCapabilities::from_device_capability_bits(
                crate::sink::BIND_CAPABILITY_BITS,
            ),
            state: SessionState::Emulating,
            ..Self::default()
        }
    }
}

/// The fake streaming session: a deterministic in-memory implementation of
/// [`crate::streaming::StreamingOutput`] that records every call and honors
/// scripted failures. **It never emits real desktop input.**
#[derive(Debug, Clone)]
pub struct FakeStreamingOutput {
    /// The shared observable state.
    pub state: Rc<RefCell<FakeStreamingState>>,
}

impl FakeStreamingOutput {
    /// Creates a fake session over a shared state cell.
    #[must_use]
    pub fn new(state: Rc<RefCell<FakeStreamingState>>) -> Self {
        Self { state }
    }
}

impl OutputSink for FakeStreamingOutput {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        let mut st = self.state.borrow_mut();
        st.submitted.push(event.clone());
        if let Some(timeline) = &st.timeline {
            timeline.borrow_mut().push("output:submit".to_string());
        }
        match st.submit_script.pop_front().unwrap_or(Ok(())) {
            Ok(()) => Ok(()),
            Err(error) => Err(OutputError::Io(error.to_string())),
        }
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        let mut st = self.state.borrow_mut();
        st.release_calls += 1;
        if let Some(timeline) = &st.timeline {
            timeline.borrow_mut().push("output:release_all".to_string());
        }
        // Faithful lifecycle (M10 review R3): like the real
        // `PortalOutputSink::release_all_detailed`, the release **clears any
        // observed server interruption** — so the coordinator must capture
        // `take_server_interruption` BEFORE `release_all` or the structured
        // category is lost — and records the cleanup error only on failure
        // (clearing it on success).
        st.interruption = None;
        match st.release_result.clone() {
            Ok(()) => {
                st.cleanup_error = None;
                Ok(())
            }
            Err(error) => {
                st.cleanup_error = Some(error.clone());
                Err(OutputError::Fatal(error.to_string()))
            }
        }
    }
}

impl crate::streaming::StreamingOutput for FakeStreamingOutput {
    fn prepare(
        &mut self,
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<OutputCapabilities, DesktopOutputError> {
        let mut st = self.state.borrow_mut();
        st.prepare_calls += 1;
        if let Some(timeline) = &st.timeline {
            timeline.borrow_mut().push("output:prepare".to_string());
        }
        st.prepare_result.clone()
    }

    fn capabilities(&self) -> OutputCapabilities {
        self.state.borrow().capabilities
    }

    fn state(&self) -> SessionState {
        self.state.borrow().state
    }

    fn take_server_interruption(&mut self) -> Option<DesktopOutputError> {
        self.state.borrow_mut().interruption.take()
    }

    fn take_cleanup_error(&mut self) -> Option<DesktopOutputError> {
        self.state.borrow_mut().cleanup_error.take()
    }
}

/// The fake streaming factory: hands out one scripted session per `create`
/// call. Tests assert `create_calls` (e.g. zero output creation when the
/// device open fails).
#[derive(Debug, Default)]
pub struct FakeStreamingOutputFactory {
    /// Scripted sessions; each `create` pops the next one.
    pub sessions: VecDeque<Rc<RefCell<FakeStreamingState>>>,
    /// How many times `create` was called.
    pub create_calls: usize,
}

impl FakeStreamingOutputFactory {
    /// Creates a factory holding one scripted session state.
    #[must_use]
    pub fn one(state: Rc<RefCell<FakeStreamingState>>) -> Self {
        Self {
            sessions: VecDeque::from([state]),
            create_calls: 0,
        }
    }
}

impl crate::streaming::StreamingOutputFactory for FakeStreamingOutputFactory {
    fn create(&mut self) -> Result<Box<dyn crate::streaming::StreamingOutput>, DesktopOutputError> {
        self.create_calls += 1;
        let state = self
            .sessions
            .pop_front()
            .expect("no fake streaming session scripted for create()");
        Ok(Box::new(FakeStreamingOutput::new(state)))
    }
}

// Keep the module's imports honest (device_types is used by tests of the
// pointer-only selection below).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::OutputCapabilities;
    use crate::portal::device_types;

    #[test]
    fn fake_portal_records_device_selection_and_close() {
        let mut portal = FakePortal::success();
        let session = portal.create_session().unwrap();
        portal
            .select_devices(&session, device_types::POINTER)
            .unwrap();
        portal.start(&session).unwrap();
        let fd = portal.connect_to_eis(&session).unwrap();
        assert_eq!(fd, EisFd(42));
        portal.close_session(&session).unwrap();
        assert_eq!(portal.selected_types, vec![device_types::POINTER]);
        assert_eq!(portal.close_calls, 1);
        assert_eq!(portal.sessions_created, 1);
    }

    #[test]
    fn fake_portal_fault_injection() {
        let mut portal = FakePortal::success();
        portal.fail_step = Some((
            FakePortalStep::Start,
            DesktopOutputError::AuthorizationCancelled,
        ));
        let session = portal.create_session().unwrap();
        assert!(portal.start(&session).is_err());
    }

    #[test]
    fn fake_transport_scripted_events_are_consumed_in_order() {
        let device = DeviceId(3);
        let mut transport = FakeTransport::happy_handshake(device);
        assert_eq!(
            transport.wait_event(std::time::Duration::ZERO).unwrap(),
            TransportEvent::Connected
        );
        assert!(matches!(
            transport.wait_event(std::time::Duration::ZERO).unwrap(),
            TransportEvent::SeatAdded { .. }
        ));
        assert!(matches!(
            transport.wait_event(std::time::Duration::ZERO).unwrap(),
            TransportEvent::DeviceAdded { .. }
        ));
        assert_eq!(
            transport.wait_event(std::time::Duration::ZERO).unwrap(),
            TransportEvent::DeviceResumed { device }
        );
        // Exhausted script -> Timeout, never an error.
        assert_eq!(
            transport.wait_event(std::time::Duration::ZERO).unwrap(),
            TransportEvent::Timeout
        );
    }

    #[test]
    fn fake_transport_records_wire_calls_and_honors_send_errors() {
        let device = DeviceId(3);
        let mut transport = FakeTransport::happy_handshake(device);
        transport.pointer_motion(device, 1.0, 2.0).unwrap();
        transport.frame(device).unwrap();
        transport.send_error = Some(DesktopOutputError::TransportDisconnected("x".into()));
        assert!(transport.button(device, 0x110, true).is_err());
        assert_eq!(
            transport.log(),
            &[
                FakeWireCall::PointerMotion {
                    device,
                    dx: 1.0,
                    dy: 2.0
                },
                FakeWireCall::Frame { device },
                FakeWireCall::Button {
                    device,
                    button: 0x110,
                    is_press: true
                },
            ]
        );
    }

    #[test]
    fn fake_output_capabilities_derive_from_bits() {
        let caps = OutputCapabilities::from_device_capability_bits(1 << 0 | 1 << 4);
        assert!(caps.relative_pointer);
        assert!(caps.pixel_scroll);
        assert!(!caps.primary_button);
    }
}
