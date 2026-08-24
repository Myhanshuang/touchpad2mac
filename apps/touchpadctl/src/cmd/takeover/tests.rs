//! M10 takeover command tests (all fake-backed, no sleeps, no real device,
//! no real portal/libei, no desktop input).
//!
//! Everything drives the mockable [`MockSys`] seam, a fake streaming output
//! session, a fake clock/readiness (time passes only through the scripted
//! readiness polls — never through `std::thread::sleep`), and a shared
//! timeline so ordering assertions are exact. The M10 test matrix
//! (M10_TASK.md §9) is covered here plus the args/bridge/streaming unit
//! tests in their own crates.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use touchpad_core::{Monotonic, OutputEvent};
use touchpad_desktop::fake::{FakeStreamingOutput, FakeStreamingState};
use touchpad_desktop::{DesktopOutputError, OutputCapabilities, StreamingOutput};
use touchpad_linux::sys::mock::{MockCall, MockDevice, MockFailure, MockSys};
use touchpad_linux::sys::{Fd, Sys, SysError};
use touchpad_linux::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, BTN_LEFT, BTN_RIGHT,
    EV_ABS, EV_KEY, EV_SYN, SYN_DROPPED, SYN_REPORT,
};

use super::*;
use crate::env::{CommandEnv, RecorderFactory, TakeoverSeams};
use crate::exit::ExitCode;

#[test]
fn momentum_tick_clock_maps_process_relative_time_into_input_epoch() {
    let mut clock = InputDomainTickClock::default();

    // Reproduces the live failure shape: evdev is ~150 s since boot while
    // the process-relative scheduling clock is only ~24 s since startup.
    clock.observe_input(
        Monotonic::from_nanos(24_000_000_000),
        Some(1),
        Some(Monotonic::from_nanos(150_000_000_000)),
    );

    assert_eq!(
        clock.map_process_now(Monotonic::from_nanos(24_220_015_246)),
        Some(Monotonic::from_nanos(150_220_015_246))
    );
}

#[test]
fn momentum_tick_clock_reanchors_after_newer_input_frame() {
    let mut clock = InputDomainTickClock::default();
    clock.observe_input(
        Monotonic::from_nanos(24_000_000_000),
        Some(1),
        Some(Monotonic::from_nanos(150_000_000_000)),
    );
    clock.observe_input(
        Monotonic::from_nanos(25_000_000_000),
        Some(2),
        Some(Monotonic::from_nanos(151_500_000_000)),
    );

    assert_eq!(
        clock.map_process_now(Monotonic::from_nanos(25_100_000_000)),
        Some(Monotonic::from_nanos(151_600_000_000))
    );
    // A regression of the scheduling seam does not fabricate negative time.
    assert_eq!(
        clock.map_process_now(Monotonic::from_nanos(24_999_999_999)),
        None
    );
}

#[test]
fn momentum_tick_clock_does_not_reanchor_without_a_new_frame_sequence() {
    let mut clock = InputDomainTickClock::default();
    clock.observe_input(
        Monotonic::from_nanos(24_000_000_000),
        Some(7),
        Some(Monotonic::from_nanos(150_000_000_000)),
    );
    // A later evdev read may contain only part of a frame. The arbiter's
    // accepted frame marker is unchanged, so elapsed momentum time must not
    // be reset to zero.
    clock.observe_input(
        Monotonic::from_nanos(24_100_000_000),
        Some(7),
        Some(Monotonic::from_nanos(150_000_000_000)),
    );
    assert_eq!(
        clock.map_process_now(Monotonic::from_nanos(24_200_000_000)),
        Some(Monotonic::from_nanos(150_200_000_000))
    );
}

/// The device path every takeover test uses.
fn device_path() -> PathBuf {
    PathBuf::from("/dev/input/event0")
}

/// Builds a mock touchpad with physical Left/Right buttons.
fn mock_touchpad() -> MockDevice {
    let mut device = MockDevice::touchpad("Pad", 10);
    device.mt_slots.insert(ABS_MT_TRACKING_ID, vec![-1; 10]);
    device.mt_slots.insert(ABS_MT_POSITION_X, vec![0; 10]);
    device.mt_slots.insert(ABS_MT_POSITION_Y, vec![0; 10]);
    device.add_key(BTN_LEFT);
    device.add_key(BTN_RIGHT);
    device
}

fn ev(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
    touchpad_linux::encode_input_event(sec, usec, event_type, code, value)
}

fn syn(sec: i64, usec: i64) -> Vec<u8> {
    ev(sec, usec, EV_SYN, SYN_REPORT, 0)
}

/// One frame with a single live contact at raw (x, y) and the given physical
/// left-button state.
fn one_frame(sec: i64, usec: i64, tid: i32, slot: u32, x: i32, y: i32, left: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_SLOT, slot as i32));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_TRACKING_ID, tid));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_X, x));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_Y, y));
    out.extend(ev(sec, usec, EV_KEY, BTN_LEFT, i32::from(left)));
    out.extend(syn(sec, usec));
    out
}

/// One frame with a single live contact at raw (x, y) and the given physical
/// left/right button states (M10 review R5: a legitimate simultaneous
/// physical Left+Right hold drives the multi-explicit-release failure case).
#[allow(clippy::too_many_arguments)]
fn buttons_frame(
    sec: i64,
    usec: i64,
    tid: i32,
    slot: u32,
    x: i32,
    y: i32,
    left: bool,
    right: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_SLOT, slot as i32));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_TRACKING_ID, tid));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_X, x));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_Y, y));
    out.extend(ev(sec, usec, EV_KEY, BTN_LEFT, i32::from(left)));
    out.extend(ev(sec, usec, EV_KEY, BTN_RIGHT, i32::from(right)));
    out.extend(syn(sec, usec));
    out
}

/// One frame that ends the contact on `slot`.
fn end_frame(sec: i64, usec: i64, slot: u32, left: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_SLOT, slot as i32));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_TRACKING_ID, -1));
    out.extend(ev(sec, usec, EV_KEY, BTN_LEFT, i32::from(left)));
    out.extend(syn(sec, usec));
    out
}

/// One frame with two live contacts at raw positions and the given physical
/// left-button state.
#[allow(clippy::too_many_arguments)]
fn two_frame(
    sec: i64,
    usec: i64,
    tid_a: i32,
    x_a: i32,
    y_a: i32,
    tid_b: i32,
    x_b: i32,
    y_b: i32,
    left: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_SLOT, 0));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_TRACKING_ID, tid_a));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_X, x_a));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_Y, y_a));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_SLOT, 1));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_TRACKING_ID, tid_b));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_X, x_b));
    out.extend(ev(sec, usec, EV_ABS, ABS_MT_POSITION_Y, y_b));
    out.extend(ev(sec, usec, EV_KEY, BTN_LEFT, i32::from(left)));
    out.extend(syn(sec, usec));
    out
}

/// The shared test harness: mock sys + fake clock/readiness/sleeper + fake
/// streaming session + shared timeline, all wired into a [`CommandEnv`].
struct Harness {
    sys: Rc<MockSys>,
    timeline: Rc<RefCell<Vec<String>>>,
    now: Rc<RefCell<Monotonic>>,
    readiness_script: Rc<RefCell<VecDeque<bool>>>,
    out: Vec<u8>,
    err: Vec<u8>,
    stop_flag: Arc<AtomicBool>,
    streaming_state: Rc<RefCell<FakeStreamingState>>,
    recorder_factory: Option<RecorderFactory>,
    sleeper_calls: Rc<RefCell<usize>>,
    /// How many times the fake streaming factory's `create` ran (object
    /// allocation — M10 review R6: allocation is side-effect-free, the
    /// external work happens in `prepare`).
    create_calls: Rc<RefCell<usize>>,
}

impl Harness {
    fn new(streaming_state: FakeStreamingState) -> Self {
        Self {
            sys: Rc::new(MockSys::new()),
            timeline: Rc::new(RefCell::new(Vec::new())),
            now: Rc::new(RefCell::new(Monotonic::ZERO)),
            readiness_script: Rc::new(RefCell::new(VecDeque::new())),
            out: Vec::new(),
            err: Vec::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
            streaming_state: Rc::new(RefCell::new(streaming_state)),
            recorder_factory: None,
            sleeper_calls: Rc::new(RefCell::new(0)),
            create_calls: Rc::new(RefCell::new(0)),
        }
    }

    /// A happy harness: full-capability streaming session, everything
    /// succeeds.
    fn happy() -> Self {
        Self::new(FakeStreamingState::happy())
    }

    /// Registers the mock device at the standard path.
    fn with_device(&mut self, device: MockDevice) -> &mut Self {
        self.sys.add_device(device_path(), device);
        self
    }

    /// Scripts the readiness outcomes; a `None`/exhausted poll returns
    /// `false` (idle) and advances the fake clock by the poll quantum
    /// (modeling the poll wait without sleeping).
    fn with_readiness(&mut self, script: Vec<bool>) -> &mut Self {
        *self.readiness_script.borrow_mut() = script.into();
        self
    }

    /// Attaches a shared-timeline recorder factory (markers: create/flush/
    /// record/finish).
    fn with_marker_recorder(&mut self) -> &mut Self {
        let timeline = Rc::clone(&self.timeline);
        self.recorder_factory = Some(Box::new(move |_, _| {
            timeline.borrow_mut().push("recorder:create".to_string());
            Ok(Box::new(MarkerRecorder {
                timeline: Rc::clone(&timeline),
                events: 0,
            }))
        }));
        self
    }

    /// Builds the command environment (one use per test).
    fn env(&mut self) -> CommandEnv<'_> {
        // Wire the shared timeline into the fake streaming session so its
        // prepare/submit/release markers participate in ordering assertions.
        self.streaming_state.borrow_mut().timeline = Some(Rc::clone(&self.timeline));
        let now = Rc::clone(&self.now);
        let script = Rc::clone(&self.readiness_script);
        let readiness: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
            Rc::new(move |_fd: Fd, timeout: Duration| {
                let ready = script.borrow_mut().pop_front().unwrap_or(false);
                if !ready {
                    let next = now
                        .borrow()
                        .checked_add(timeout)
                        .unwrap_or(Monotonic::from_nanos(u64::MAX));
                    *now.borrow_mut() = next;
                }
                Ok(ready)
            });
        let now_clock = Rc::clone(&self.now);
        let clock: Rc<dyn Fn() -> Monotonic> = Rc::new(move || *now_clock.borrow());
        let sleeper_calls = Rc::clone(&self.sleeper_calls);
        let sleeper_timeline = Rc::clone(&self.timeline);
        let sleeper: Rc<dyn Fn(Duration)> = Rc::new(move |_d: Duration| {
            *sleeper_calls.borrow_mut() += 1;
            sleeper_timeline
                .borrow_mut()
                .push("countdown:sleep".to_string());
        });
        let state = Rc::clone(&self.streaming_state);
        let state_for_factory = Rc::clone(&self.streaming_state);
        let create_calls = Rc::clone(&self.create_calls);
        let factory: Box<dyn FnMut() -> Result<Box<dyn StreamingOutput>, DesktopOutputError>> =
            Box::new(move || {
                let _ = &state;
                *create_calls.borrow_mut() += 1;
                Ok(Box::new(FakeStreamingOutput::new(Rc::clone(
                    &state_for_factory,
                ))))
            });
        CommandEnv {
            sys: Rc::new(TimelineSys {
                inner: Rc::clone(&self.sys),
                timeline: Rc::clone(&self.timeline),
            }) as Rc<dyn touchpad_linux::sys::Sys>,
            out: &mut self.out,
            err: &mut self.err,
            stop_flag: Arc::clone(&self.stop_flag),
            recorder_factory: self.recorder_factory.take(),
            output_factory: None,
            takeover: TakeoverSeams {
                clock,
                readiness,
                sleeper,
                streaming_factory: Some(factory),
                real_desktop_backend: crate::env::RealDesktopBackend::PortalLibei,
            },
        }
    }
}

/// A recorder that records markers into the shared timeline.
struct MarkerRecorder {
    timeline: Rc<RefCell<Vec<String>>>,
    events: u64,
}

impl touchpad_linux::RawEventRecorder for MarkerRecorder {
    fn record(&mut self, _event: &touchpad_linux::KernelEvent) -> Result<(), RecorderError> {
        self.timeline
            .borrow_mut()
            .push("recorder:record".to_string());
        self.events += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RecorderError> {
        self.timeline
            .borrow_mut()
            .push("recorder:flush".to_string());
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RecorderError> {
        self.timeline
            .borrow_mut()
            .push("recorder:finish".to_string());
        Ok(())
    }

    fn events_recorded(&self) -> u64 {
        self.events
    }
}

/// A sys seam that records a marker for every call into the shared timeline,
/// delegating behavior to the `MockSys`.
struct TimelineSys {
    inner: Rc<MockSys>,
    timeline: Rc<RefCell<Vec<String>>>,
}

impl touchpad_linux::sys::Sys for TimelineSys {
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("read_dir({})", path.display()));
        self.inner.read_dir(path)
    }

    fn open(&self, path: &Path) -> Result<Fd, SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("open({})", path.display()));
        self.inner.open(path)
    }

    fn close(&self, fd: Fd) -> Result<(), SysError> {
        self.timeline.borrow_mut().push(format!("close({fd:?})"));
        self.inner.close(fd)
    }

    fn read(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        self.timeline.borrow_mut().push(format!("read({fd:?})"));
        self.inner.read(fd, buf)
    }

    fn ioctl_grab(&self, fd: Fd, grab: bool) -> Result<(), SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("grab({fd:?}, {grab})"));
        self.inner.ioctl_grab(fd, grab)
    }

    fn ioctl_set_clock_id(&self, fd: Fd, clock_id: u32) -> Result<(), SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("clock({fd:?}, {clock_id})"));
        self.inner.ioctl_set_clock_id(fd, clock_id)
    }

    fn ioctl_name(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        self.timeline.borrow_mut().push(format!("name({fd:?})"));
        self.inner.ioctl_name(fd, buf)
    }

    fn ioctl_id(&self, fd: Fd) -> Result<touchpad_linux::sys::InputId, SysError> {
        self.timeline.borrow_mut().push(format!("id({fd:?})"));
        self.inner.ioctl_id(fd)
    }

    fn ioctl_ev_bits(&self, fd: Fd, ev_type: u16, buf: &mut [u8]) -> Result<usize, SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("evbits({fd:?}, {ev_type})"));
        self.inner.ioctl_ev_bits(fd, ev_type, buf)
    }

    fn ioctl_prop_bits(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        self.timeline.borrow_mut().push(format!("propbits({fd:?})"));
        self.inner.ioctl_prop_bits(fd, buf)
    }

    fn ioctl_key_state(&self, fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
        self.timeline.borrow_mut().push(format!("keystate({fd:?})"));
        self.inner.ioctl_key_state(fd, buf)
    }

    fn ioctl_absinfo(
        &self,
        fd: Fd,
        abs_code: u16,
    ) -> Result<touchpad_linux::sys::AbsInfo, SysError> {
        self.timeline
            .borrow_mut()
            .push(format!("absinfo({fd:?}, {abs_code})"));
        self.inner.ioctl_absinfo(fd, abs_code)
    }

    fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError> {
        self.timeline.borrow_mut().push(format!("mtslots({fd:?})"));
        self.inner.ioctl_mt_slots(fd, buf)
    }

    fn poll(&self, fd: Fd, timeout: Duration) -> Result<bool, SysError> {
        self.timeline.borrow_mut().push(format!("poll({fd:?})"));
        self.inner.poll(fd, timeout)
    }
}

fn run_takeover(env: &mut CommandEnv<'_>, duration: u32) -> Result<(), CommandFailure> {
    super::run(
        env,
        &device_path(),
        &temp_trace("run"),
        duration,
        "m10-linear-v1",
        ProfileInputs::default(),
    )
}

/// The position of the first marker matching a predicate.
fn pos(timeline: &[String], marker: &str) -> usize {
    timeline
        .iter()
        .position(|m| m.contains(marker))
        .unwrap_or_else(|| panic!("marker {marker:?} not in timeline: {timeline:?}"))
}

// ---------------------------------------------------------------------------
// Preparation order and the bounded loop (M10_TASK.md §4/§7)
// ---------------------------------------------------------------------------

/// The exact success startup timeline: device open/validate → output ready →
/// recorder header flush → countdown complete → grab → first read, then a
/// deadline stop with the ordered cleanup (output release → recorder finish →
/// ungrab → close) and exit 0.
#[test]
fn success_startup_timeline_deadline_stop_and_ordered_cleanup() {
    let mut device = mock_touchpad();
    // One pointer move: begin (1.0,1.0)mm → (2.0,1.0)mm commits a 10 px move.
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false));
    batch.extend(one_frame(1, 1100, 10, 0, 200, 100, false));
    batch.extend(end_frame(1, 1200, 0, false));
    device.push_raw(batch);

    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    // First poll ready (the events), then idle (deadline expires).
    h.with_readiness(vec![true]);

    let mut env = h.env();
    let result = run_takeover(&mut env, 1);
    // The deadline stop with all cleanup succeeded is a clean exit 0.
    assert!(result.is_ok(), "{result:?}");
    drop(env);
    let err_text = String::from_utf8(h.err).unwrap();
    assert!(err_text.contains("maximum duration reached"), "{err_text}");

    let timeline = h.timeline.borrow();
    let open = pos(&timeline, "open(");
    let prepare = pos(&timeline, "output:prepare");
    let flush = pos(&timeline, "recorder:flush");
    let countdown = pos(&timeline, "countdown:sleep");
    let grab = pos(&timeline, ", true)");
    let read = pos(&timeline, "read(");
    assert!(
        open < prepare && prepare < flush && flush < countdown && countdown < grab && grab < read,
        "startup order: {timeline:?}"
    );
    // Ordered cleanup: output release → recorder finish → ungrab → close.
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let ungrab = pos(&timeline, ", false)");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < ungrab && ungrab < close,
        "cleanup order: {timeline:?}"
    );
    // Exactly one grab and one ungrab.
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    // The fake output received the resolved pointer move.
    let submitted = h.streaming_state.borrow().submitted.clone();
    assert_eq!(
        submitted,
        vec![OutputEvent::PointerMove {
            dx: touchpad_core::LogicalPixels::try_new(10.0).unwrap(),
            dy: touchpad_core::LogicalPixels::try_new(0.0).unwrap(),
        }],
        "the pointer move must travel decoder → arbiter → output"
    );
}

/// The maximum duration expires with an entirely idle device at both
/// boundaries (1 and 300 seconds) — fake clock, no sleeps.
#[test]
fn max_duration_expires_with_idle_device_at_1_and_300_seconds() {
    for duration in [1u32, 300u32] {
        let mut h = Harness::happy();
        h.with_device(mock_touchpad());
        h.with_readiness(vec![]); // never ready → idle
        let mut env = h.env();
        let result = run_takeover(&mut env, duration);
        assert!(result.is_ok(), "duration {duration}: {result:?}");
        let err_text = String::from_utf8(h.err).unwrap();
        assert!(
            err_text.contains("maximum duration reached"),
            "duration {duration}: {err_text}"
        );
        // No read ever happened (the idle device produced no input).
        assert_eq!(h.sys.count(|call| matches!(call, MockCall::Read(..))), 0);
        // The countdown ran (no-op sleeper) and the cleanup was ordered.
        assert_eq!(*h.sleeper_calls.borrow(), 3);
        let timeline = h.timeline.borrow();
        let release = pos(&timeline, "output:release_all");
        let close = pos(&timeline, "close(");
        assert!(release < close, "{timeline:?}");
    }
}

/// A signal stop during the loop (injectable stop flag) is a clean stop
/// (exit 0) with the ordered cleanup.
#[test]
fn signal_during_loop_is_a_clean_stop() {
    let mut device = mock_touchpad();
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false));
    device.push_raw(batch);
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    // The stop is requested DURING the loop (after the grab and the first
    // read): the readiness closure requests the stop on its second poll, so
    // the loop's next top-of-iteration check observes it.
    let stop = Arc::clone(&h.stop_flag);
    let polls = Rc::new(RefCell::new(0usize));
    let scripted: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
        Rc::new(move |_fd: Fd, _t: Duration| {
            let n = {
                let mut n = polls.borrow_mut();
                *n += 1;
                *n
            };
            if n == 1 {
                Ok(true)
            } else {
                stop.store(true, Ordering::Relaxed);
                Ok(false)
            }
        });
    let mut env = h.env();
    env.takeover.readiness = scripted;
    let result = run_takeover(&mut env, 60);
    assert!(result.is_ok(), "{result:?}");
    drop(env);
    let err_text = String::from_utf8(h.err).unwrap();
    assert!(
        err_text.contains("SIGINT/SIGTERM (controlled stop)"),
        "{err_text}"
    );
    // The grab was acquired and released exactly once.
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

/// M10 review R1: a `poll(2)` interrupted by a signal **while a stop was
/// requested** (Ctrl-C/SIGTERM with the non-`SA_RESTART` handler while the
/// loop is idle) is the documented controlled stop — clean exit 0 with the
/// ordered cleanup — NOT a stream failure.
#[test]
fn poll_eintr_with_requested_stop_is_a_clean_signal_stop() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    // The readiness seam surfaces the real EINTR of an interrupted poll(2)
    // AND requests the stop at the same moment — exactly like a real Ctrl-C
    // with the non-`SA_RESTART` handler: the handler records the stop and
    // the pending signal interrupts the idle poll. The stop is requested
    // only inside the loop (after the countdown and the grab), so the
    // pre-grab path is not aborted.
    let stop = Arc::clone(&h.stop_flag);
    let readiness: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
        Rc::new(move |_fd: Fd, _t: Duration| {
            stop.store(true, Ordering::Relaxed);
            Err(SysError::Interrupted)
        });
    let mut env = h.env();
    env.takeover.readiness = readiness;
    let result = run_takeover(&mut env, 300);
    assert!(result.is_ok(), "{result:?}");
    drop(env);
    let err_text = String::from_utf8(h.err).unwrap();
    assert!(
        err_text.contains("SIGINT/SIGTERM (controlled stop)"),
        "{err_text}"
    );
    // The ordered cleanup ran: output release → recorder finish → ungrab →
    // close, exactly one grab and one ungrab.
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let ungrab = pos(&timeline, ", false)");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < ungrab && ungrab < close,
        "cleanup order: {timeline:?}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

/// M10 review R1: an **unrequested** `poll(2)` EINTR (no stop requested)
/// keeps its M4/M5 semantics as an actionable poll/stream failure (exit 6),
/// with the ordered cleanup still running.
#[test]
fn poll_eintr_without_requested_stop_is_a_stream_failure() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    // No stop requested; the readiness seam returns a genuine EINTR.
    let readiness: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
        Rc::new(|_fd: Fd, _t: Duration| Err(SysError::Interrupted));
    let mut env = h.env();
    env.takeover.readiness = readiness;
    let failure = run_takeover(&mut env, 300).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(
        failure.to_string().contains("EINTR"),
        "the unrequested EINTR must stay a structured poll failure: {failure}"
    );
    assert!(!failure.to_string().contains("SIGINT/SIGTERM"), "{failure}");
    // The ordered cleanup still ran (output release before the device close).
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let close = pos(&timeline, "close(");
    assert!(release < close, "{timeline:?}");
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

// ---------------------------------------------------------------------------
// Pre-grab failures: zero grabs, ordered release of what exists
// ---------------------------------------------------------------------------

/// A device-open failure causes no output preparation (the fake session's
/// `prepare` — the external work — never runs) and no grab; the open path
/// itself closes nothing (the device was never opened). M10 review R6: the
/// session factory's `create` is pure object allocation (side-effect-free,
/// like the real lazy factory), so only the allocation happens; the external
/// preparation — and with it all D-Bus/libei/output access — stays at zero,
/// and the **device-error precedence** is retained (exit 2, not an output
/// failure).
#[test]
fn device_open_failure_causes_no_output_preparation_or_grab() {
    let mut h = Harness::happy(); // no device registered → open fails
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    // Device-error precedence: the missing device (exit 2) is reported, NOT
    // any output failure.
    assert_eq!(failure.exit_code(), ExitCode::InputDir, "{failure}");
    assert!(
        failure.to_string().contains("no such device node"),
        "{failure}"
    );
    let state = h.streaming_state.borrow();
    assert_eq!(
        state.prepare_calls, 0,
        "no output preparation on device-open failure"
    );
    // The allocation (create) happened exactly once; the external work
    // (prepare) never did — the observable factory/preparation timeline.
    assert_eq!(*h.create_calls.borrow(), 1, "one session object allocated");
    assert_eq!(state.prepare_calls, 0, "zero external preparation");
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
}

/// An output preparation failure releases the (partially) prepared session
/// and closes the device with zero grabs and zero recorder events.
#[test]
fn output_prepare_failure_releases_session_closes_device_zero_grab() {
    let mut state = FakeStreamingState::happy();
    state.prepare_result = Err(DesktopOutputError::AuthorizationCancelled);
    let mut h = Harness::new(state);
    h.with_device(mock_touchpad());
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Permission, "{failure}");
    // The prepared session was explicitly released (wrapped release_all ran).
    let st = h.streaming_state.borrow();
    assert_eq!(st.release_calls, 1, "the session must be released");
    assert_eq!(st.prepare_calls, 1);
    // Zero grabs; zero recorder events; the device fd was closed.
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A missing negotiated capability refuses before the recorder and the grab.
#[test]
fn capability_missing_refuses_before_recorder_and_grab() {
    // Pointer-only session: no buttons, no scroll.
    let mut state = FakeStreamingState::happy();
    state.prepare_result = Ok(OutputCapabilities::from_device_capability_bits(1 << 0));
    state.capabilities = OutputCapabilities::from_device_capability_bits(1 << 0);
    let mut h = Harness::new(state);
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::NoCandidate, "{failure}");
    assert!(failure.to_string().contains("capability"), "{failure}");
    // The recorder was never created and no grab was issued.
    let timeline = h.timeline.borrow();
    assert!(
        !timeline.iter().any(|m| m.contains("recorder:create")),
        "the recorder must not be created: {timeline:?}"
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A recorder header-flush failure after output ready: the output releases
/// before the device close, the recorder is finalized, zero grabs.
#[test]
fn recorder_header_flush_failure_releases_output_before_device_close_zero_grab() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    let timeline = Rc::clone(&h.timeline);
    h.recorder_factory = Some(Box::new(move |_, _| {
        timeline.borrow_mut().push("recorder:create".to_string());
        Ok(Box::new(FlushFailingRecorder))
    }));
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
    let timeline = h.timeline.borrow();
    let prepare = pos(&timeline, "output:prepare");
    let release = pos(&timeline, "output:release_all");
    let close = pos(&timeline, "close(");
    assert!(
        prepare < release && release < close,
        "output release must precede the device close: {timeline:?}"
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A countdown cancel (injectable stop during the countdown): nothing was
/// grabbed, the prepared output session was released, the recorder
/// finalized, the device closed — exit 8.
#[test]
fn countdown_cancel_releases_output_finishes_recorder_closes_with_zero_grab() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    h.stop_flag.store(true, Ordering::Relaxed);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
    assert!(
        failure
            .to_string()
            .contains("aborted by the user before the takeover began"),
        "{failure}"
    );
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < close,
        "ordered cleanup: {timeline:?}"
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A status-writer failure during the countdown still runs the ordered
/// cleanup (output release → recorder finish → close) with zero grabs.
#[test]
fn status_writer_failure_during_countdown_runs_ordered_cleanup() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    // A failing status writer after a few writes.
    let mut failing_writer = FailAfterWrites { remaining: 6 };
    let mut env = h.env();
    env.err = &mut failing_writer;
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Unexpected, "{failure}");
    assert!(
        failure.to_string().contains("status output failed"),
        "{failure}"
    );
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < close,
        "ordered cleanup: {timeline:?}"
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Grab(..))), 0);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A `Write` that fails after `remaining` successful writes.
struct FailAfterWrites {
    remaining: usize,
}

impl std::io::Write for FailAfterWrites {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("injected status failure"));
        }
        self.remaining -= 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A recorder whose header flush always fails.
struct FlushFailingRecorder;

impl touchpad_linux::RawEventRecorder for FlushFailingRecorder {
    fn record(&mut self, _event: &touchpad_linux::KernelEvent) -> Result<(), RecorderError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RecorderError> {
        Err(RecorderError::Trace(
            touchpad_trace::TraceError::InvalidState("injected flush failure"),
        ))
    }

    fn finish(&mut self) -> Result<(), RecorderError> {
        Ok(())
    }

    fn events_recorded(&self) -> u64 {
        0
    }
}

/// A recorder whose `record` always fails (recorder event failure during the
/// loop).
struct RecordFailingRecorder;

impl touchpad_linux::RawEventRecorder for RecordFailingRecorder {
    fn record(&mut self, _event: &touchpad_linux::KernelEvent) -> Result<(), RecorderError> {
        Err(RecorderError::Trace(
            touchpad_trace::TraceError::InvalidState("injected record failure"),
        ))
    }

    fn flush(&mut self) -> Result<(), RecorderError> {
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RecorderError> {
        Ok(())
    }

    fn events_recorded(&self) -> u64 {
        0
    }
}

/// A recorder whose `finish` always fails.
struct FinishFailingRecorder;

impl touchpad_linux::RawEventRecorder for FinishFailingRecorder {
    fn record(&mut self, _event: &touchpad_linux::KernelEvent) -> Result<(), RecorderError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), RecorderError> {
        Ok(())
    }

    fn finish(&mut self) -> Result<(), RecorderError> {
        Err(RecorderError::Trace(
            touchpad_trace::TraceError::InvalidState("injected finish failure"),
        ))
    }

    fn events_recorded(&self) -> u64 {
        0
    }
}

// ---------------------------------------------------------------------------
// Loop-time failures (deferred cleanup keeps the resources for the
// coordinator's ordered shutdown)
// ---------------------------------------------------------------------------

/// Device EOF/unplug during the loop: a stream failure (exit 6) with the
/// ordered cleanup — the output is released before the recorder finish and
/// the device release.
#[test]
fn device_eof_unplug_is_reported_with_ordered_cleanup() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    // One poll ready → the read returns EOF (empty stream).
    h.with_readiness(vec![true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(failure.to_string().contains("disconnected"), "{failure}");
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let ungrab = pos(&timeline, ", false)");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < ungrab && ungrab < close,
        "ordered cleanup: {timeline:?}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

/// A readiness/poll error during the loop is a stream failure with the
/// ordered cleanup (the device fd is still released).
#[test]
fn readiness_error_is_a_stream_failure() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    let script = Rc::new(RefCell::new(VecDeque::new()));
    script
        .borrow_mut()
        .push_back(Err(SysError::Io(std::io::Error::other("poll failed"))));
    let now = h.now.clone();
    let readiness: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
        Rc::new(move |_fd: Fd, _t: Duration| script.borrow_mut().pop_front().unwrap_or(Ok(false)));
    let clock = {
        let now = Rc::clone(&now);
        Rc::new(move || *now.borrow())
    };
    let mut env = h.env();
    env.takeover.readiness = readiness;
    env.takeover.clock = clock;
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(failure.to_string().contains("poll failed"), "{failure}");
    // The device was still released in the ordered shutdown.
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// M10 review R2: a hung-up device (`POLLHUP` — an unplugged/failed fd that
/// wakes without `POLLIN`) must wake the bounded loop immediately and
/// surface the EOF, initiating the ordered cleanup **without waiting for the
/// deadline**. The faithful mock `poll` is the readiness seam, so a loop
/// that treated HUP as idle would never exit (the fake clock stays at zero —
/// no idle polls advance it).
#[test]
fn unplug_hangup_wakes_the_loop_and_cleans_up_without_waiting_for_deadline() {
    let mut device = mock_touchpad();
    // Unplugged with an exhausted stream: the fd still reports hangup.
    device.poll_hup = true;
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    let sys = Rc::clone(&h.sys);
    let mut env = h.env();
    env.takeover.readiness = Rc::new(move |fd: Fd, t: Duration| sys.poll(fd, t));
    let failure = run_takeover(&mut env, 300).unwrap_err();
    // The unplug surfaces as the real EOF (exit 6), NOT the deadline.
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(failure.to_string().contains("disconnected"), "{failure}");
    // Ordered cleanup: output release → recorder finish → ungrab → close.
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let ungrab = pos(&timeline, ", false)");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < ungrab && ungrab < close,
        "cleanup order: {timeline:?}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

/// M10 review R2: `POLLNVAL` (an invalid fd) is an immediate structured
/// stream failure, never treated as idle until the deadline.
#[test]
fn poll_nval_is_an_immediate_structured_stream_failure() {
    let mut device = mock_touchpad();
    device.poll_nval = true;
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    let sys = Rc::clone(&h.sys);
    let mut env = h.env();
    env.takeover.readiness = Rc::new(move |fd: Fd, t: Duration| sys.poll(fd, t));
    let failure = run_takeover(&mut env, 300).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(
        failure.to_string().contains("POLLNVAL"),
        "the invalid fd must be an immediate structured error: {failure}"
    );
    // The ordered cleanup still ran.
    let timeline = h.timeline.borrow();
    assert!(pos(&timeline, "output:release_all") < pos(&timeline, "close("));
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A timestamp regression in the raw stream is a stream failure (the runtime
/// rejects regressing monotonic timestamps).
#[test]
fn timestamp_regression_is_a_stream_failure() {
    let mut device = mock_touchpad();
    device.push_raw(one_frame(2, 1000, 10, 0, 100, 100, false));
    device.push_raw(one_frame(1, 900, 10, 0, 200, 100, false)); // regresses
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    h.with_readiness(vec![true, true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(failure.to_string().contains("regression"), "{failure}");
    // Ordered cleanup still ran.
    let timeline = h.timeline.borrow();
    assert!(pos(&timeline, "output:release_all") < pos(&timeline, "close("));
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

/// A `SYN_DROPPED` resync failure degrades the decoder → stream failure with
/// the ordered cleanup.
#[test]
fn syn_dropped_resync_failure_is_a_stream_failure() {
    let mut device = mock_touchpad();
    let mut batch = one_frame(1, 1000, 10, 0, 100, 100, false);
    batch.extend(ev(1, 1100, EV_SYN, SYN_DROPPED, 0));
    batch.extend(syn(1, 1100));
    device.push_raw(batch);
    // The resync snapshot query fails → the decoder degrades.
    device.mt_slots_error = Some(MockFailure::Io);
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    h.with_readiness(vec![true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(
        failure.to_string().contains("resynchronization"),
        "{failure}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A recorder event failure during the loop is a recorder failure (exit 7);
/// the raw events already recorded stay recorded and the ordered cleanup
/// runs.
#[test]
fn recorder_event_failure_is_a_recorder_exit() {
    let mut device = mock_touchpad();
    device.push_raw(one_frame(1, 1000, 10, 0, 100, 100, false));
    let mut h = Harness::happy();
    h.with_device(device);
    h.recorder_factory = Some(Box::new(|_, _| Ok(Box::new(RecordFailingRecorder))));
    h.with_readiness(vec![true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
    assert!(
        failure.to_string().contains("injected record failure"),
        "{failure}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// A grab failure after all preparation succeeded: exit 6, the ordered
/// cleanup runs, and the failed release is attempted at most once (the
/// ungrab is a no-op because the grab never succeeded).
#[test]
fn grab_failure_is_reported_with_ordered_cleanup() {
    let mut device = mock_touchpad();
    device.grab_error = Some(MockFailure::Io);
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(failure.to_string().contains("grab"), "{failure}");
    // The grab was attempted exactly once (and failed); the ordered cleanup
    // released the output and closed the device.
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    let timeline = h.timeline.borrow();
    assert!(pos(&timeline, "output:release_all") < pos(&timeline, "close("));
}

// ---------------------------------------------------------------------------
// Output-fault / partial-submit behavior (M10_TASK.md §6)
// ---------------------------------------------------------------------------

/// The first output rejection faults the bridge: no later semantic/wire
/// output from the same read batch, and cleanup releases exactly the owed
/// state.
#[test]
fn first_output_rejection_blocks_later_output_and_cleans_up_owed_state() {
    let mut device = mock_touchpad();
    // A batch with several frames: pointer move, a physical left press, then
    // more motion — all in ONE read batch.
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false)); // begin
    batch.extend(one_frame(1, 1100, 10, 0, 200, 100, false)); // commit → move(10,0) — submit 1
    let pressed = one_frame(1, 1200, 10, 0, 300, 100, true); // left press — submit 2
    batch.extend(pressed);
    batch.extend(one_frame(1, 1300, 10, 0, 400, 100, true)); // move — submit 3 (REJECTED)
    device.push_raw(batch);

    let mut state = FakeStreamingState::happy();
    // Reject the 3rd submission; the accepted ButtonDown(Left) is owed.
    state.submit_script = vec![
        Ok(()),
        Ok(()),
        Err(DesktopOutputError::SendFailed(
            "injected rejection".to_string(),
        )),
    ]
    .into();
    let mut h = Harness::new(state);
    h.with_device(device);
    h.with_marker_recorder();
    h.with_readiness(vec![true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert!(
        failure.to_string().contains("semantic output fault"),
        "{failure}"
    );

    // No later semantic output from the same batch: the submitted events are
    // exactly the two accepted submissions (move, press) — the rejected one
    // and every later frame produced nothing.
    // The fake records every submitted event, including the REJECTED one (so
    // "no later wire output" is provable). The stream is: move (accepted),
    // press (accepted), move (REJECTED — the fault), then the release's owed
    // ButtonUp(Left). The later frame of the same batch (the second move
    // after the press) must NOT appear.
    let st = h.streaming_state.borrow();
    let submitted = st.submitted.clone();
    assert_eq!(
        submitted,
        vec![
            move_event(10.0, 0.0),
            down_left(),
            move_event(10.0, 0.0), // rejected (recorded for the no-late-output proof)
            up_left(),             // the owed release
        ],
        "no later output after the rejection: {submitted:?}"
    );
    // Cleanup releases exactly the owed state: one ButtonUp(Left) (the
    // accepted press), then the wrapped session release.
    let ups: Vec<_> = submitted
        .iter()
        .filter(|e| matches!(e, OutputEvent::ButtonUp(touchpad_core::MouseButton::Left)))
        .cloned()
        .collect();
    assert_eq!(ups.len(), 1, "exactly the owed left up: {submitted:?}");
    assert_eq!(st.release_calls, 1, "the wrapped cleanup runs once");
}

/// A server interruption surfaces as a structured output fault (the
/// M6-consistent transport exit 5, not the generic stream exit 6), no
/// later wire output, and the ordered cleanup still runs.
///
/// M10 review R3: the fake lifecycle is faithful to the real adapter — its
/// `release_all` clears the interruption — so this test genuinely exercises
/// the coordinator's capture-before-release order: the structured category
/// must be taken from the session BEFORE the release or it would be lost
/// (exit 6, generic stream failure) instead of the M6-consistent
/// transport-exit (exit 5).
#[test]
fn server_interruption_is_a_structured_output_fault() {
    let mut device = mock_touchpad();
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false)); // no events
    batch.extend(one_frame(1, 1100, 10, 0, 200, 100, false)); // move — submit 1 (accepted)
    batch.extend(one_frame(1, 1200, 10, 0, 300, 100, false)); // move — submit 2 (interrupted)
    device.push_raw(batch);
    let mut state = FakeStreamingState::happy();
    state.submit_script = vec![
        Ok(()),
        Err(DesktopOutputError::DevicePaused(
            "the EIS device was paused".to_string(),
        )),
    ]
    .into();
    state.interruption = Some(DesktopOutputError::DevicePaused(
        "the EIS device was paused".to_string(),
    ));
    let mut h = Harness::new(state);
    h.with_device(device);
    h.with_readiness(vec![true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
    assert!(failure.to_string().contains("paused"), "{failure}");
    // No later wire output after the interruption: exactly the accepted
    // move and the interrupted (recorded) move; the later frame of the batch
    // produced nothing.
    let st = h.streaming_state.borrow();
    assert_eq!(st.submitted.len(), 2, "{:?}", st.submitted);
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

// ---------------------------------------------------------------------------
// Cleanup failure composition (M10_TASK.md §8: every failure preserved)
// ---------------------------------------------------------------------------

/// M10 review R5: a legitimate **simultaneous physical Left+Right hold** is
/// driven through the decoder → arbiter, both downs are accepted, and then
/// **both** explicit cleanup ups are rejected, the wrapped output cleanup
/// fails, the recorder finish fails, and the ungrab and close fail. The
/// returned diagnostic identifies both failed explicit release events
/// separately and preserves every later failure with the documented
/// precedence (recorder finalize 7 > output release 7 > device release 6).
#[test]
fn multiple_cleanup_failures_preserve_all_diagnostics_and_precedence() {
    let mut device = mock_touchpad();
    // A legitimate Left+Right held state (M9 re-review 2: simultaneous
    // physical Left and Right holds): left press first, then right press
    // while left stays held, then the device is unplugged (EOF) to end the
    // loop with both buttons owed.
    let mut batch = Vec::new();
    batch.extend(buttons_frame(1, 1000, 10, 0, 100, 100, true, false)); // left down
    batch.extend(buttons_frame(1, 1100, 10, 0, 100, 100, true, true)); // right down
    device.push_raw(batch);
    device.push_eof();

    let mut state = FakeStreamingState::happy();
    // Accept BOTH downs; reject BOTH explicit cleanup ups with distinct
    // diagnostics (the first is `primary`, the second `others`), then fail
    // the wrapped cleanup.
    state.submit_script = vec![
        Ok(()),
        Ok(()),
        Err(DesktopOutputError::SendFailed(
            "explicit left up rejected".to_string(),
        )),
        Err(DesktopOutputError::SendFailed(
            "explicit right up rejected".to_string(),
        )),
    ]
    .into();
    state.release_result = Err(DesktopOutputError::TransportDisconnected(
        "wrapped cleanup failed".to_string(),
    ));
    let mut h = Harness::new(state);
    h.with_device(device);
    // Recorder finish fails, ungrab fails, close fails.
    h.recorder_factory = Some(Box::new(|_, _| Ok(Box::new(FinishFailingRecorder))));
    let device_rc = h.sys.device(&device_path()).expect("device");
    device_rc.borrow_mut().release_error = Some(MockFailure::Io);
    device_rc.borrow_mut().close_error = Some(MockFailure::Io);
    h.with_readiness(vec![true, true, true]); // two frames, then the EOF
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    // Precedence: recorder finalize failure wins (exit 7).
    assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
    let message = failure.to_string();
    // BOTH explicit release events are identified separately.
    assert!(message.contains("explicit left up rejected"), "{message}");
    assert!(message.contains("explicit right up rejected"), "{message}");
    // The wrapped cleanup failure is preserved (inside the
    // `ArbiterSinkError::ReleaseFailed` diagnostic).
    assert!(message.contains("wrapped cleanup failed"), "{message}");
    assert!(message.contains("recorder finish failed"), "{message}");
    assert!(message.contains("injected finish failure"), "{message}");
    assert!(message.contains("ungrab failed"), "{message}");
    assert!(message.contains("close failed"), "{message}");

    // The fake received exactly the two accepted downs and then the two
    // rejected cleanup ups (rejected submissions are recorded for the
    // no-late-output / owed-state proof).
    let submitted = h.streaming_state.borrow().submitted.clone();
    assert_eq!(
        submitted,
        vec![down_left(), down_right(), up_left(), up_right()],
        "exactly the accepted downs plus the owed (rejected) ups: {submitted:?}"
    );
    // The wrapped cleanup ran once.
    assert_eq!(h.streaming_state.borrow().release_calls, 1);
    // The device was still released (ungrab attempted once, close attempted).
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    assert_eq!(h.sys.count(|call| matches!(call, MockCall::Close(_))), 1);
}

/// M10 review R5 (separate success/idempotence coverage): a successful
/// cleanup after an output fault releases exactly the owed state and the
/// shutdown is a full no-op on retry — a repeated [`finalize`] on the
/// already-emptied coordinator issues zero additional output/recorder/device
/// calls.
#[test]
fn cleanup_success_releases_owed_state_and_retry_is_a_no_op() {
    let mut device = mock_touchpad();
    // A held physical Left (the release owes one ButtonUp(Left)).
    device.push_raw(buttons_frame(1, 1000, 10, 0, 100, 100, true, false));
    device.push_eof();
    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    h.with_readiness(vec![true, true]);
    let mut env = h.env();
    let failure = run_takeover(&mut env, 60).unwrap_err();
    // EOF/unplug: a stream failure, but the cleanup succeeded — the owed
    // ButtonUp(Left) reached the fake and the wrapped release succeeded.
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");

    // The coordinator ran `finalize` at the end of `run` (the guard is
    // already emptied): a second explicit `finalize` must be a full no-op —
    // zero additional output releases, recorder finishes, ungrabs, or closes
    // — and a controlled signal stop reports clean (exit 0).
    let mut guard = TakeoverCleanup {
        runtime: None,
        unattached_recorder: None,
    };
    let result = finalize(&mut env, &mut guard, StopReason::Signal);
    assert!(result.is_ok(), "{result:?}");
    drop(env);

    let submitted = h.streaming_state.borrow().submitted.clone();
    assert_eq!(submitted, vec![down_left(), up_left()]);
    let releases = h.streaming_state.borrow().release_calls;
    assert_eq!(releases, 1);
    let grabs = h.sys.count(|call| matches!(call, MockCall::Grab(_, false)));
    let closes = h.sys.count(|call| matches!(call, MockCall::Close(_)));
    assert_eq!(grabs, 1);
    assert_eq!(closes, 1);
    // The retry's own status lines are harmless; the cleanup counters prove
    // the no-op (nothing ran twice).
    assert_eq!(h.streaming_state.borrow().release_calls, releases);
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        grabs
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Close(_))),
        closes
    );
}

// ---------------------------------------------------------------------------
// Fallback Drop ordering (M10_TASK.md §8: panic/early-return fallback)
// ---------------------------------------------------------------------------

/// Dropping the coordinator's cleanup guard without the explicit finalize
/// still performs the ordered best-effort release: output session release
/// before the recorder finalization and the device release.
#[test]
fn fallback_drop_runs_ordered_cleanup() {
    let mut h = Harness::happy();
    h.with_device(mock_touchpad());
    h.with_marker_recorder();
    // Wire the timeline into the fake session so its release marker
    // participates in the ordering assertion.
    h.streaming_state.borrow_mut().timeline = Some(Rc::clone(&h.timeline));
    // Drive to the point where the guard exists and the recorder is attached,
    // then drop the guard without finalize (simulating an early return /
    // unwind after resources were acquired).
    // We emulate the coordinator's setup directly (the guard is private):
    let output = FakeStreamingOutput::new(Rc::clone(&h.streaming_state));
    let profile = M10Profile::new().unwrap();
    let bridge = TakeoverBridge::new(
        profile.arbiter_config(),
        Box::new(output) as Box<dyn StreamingOutput>,
    );
    let sys: Rc<dyn touchpad_linux::sys::Sys> = Rc::new(TimelineSys {
        inner: Rc::clone(&h.sys),
        timeline: Rc::clone(&h.timeline),
    });
    let mut runtime = EvdevRuntime::open(sys, &device_path(), bridge).unwrap();
    let factory = h
        .recorder_factory
        .take()
        .expect("marker recorder factory attached");
    let recorder = factory(
        Path::new("t.jsonl"),
        &TraceHeader::new(runtime.descriptor().unwrap().clone()),
    )
    .unwrap();
    runtime.set_recorder(recorder);
    let guard = TakeoverCleanup {
        runtime: Some(runtime),
        unattached_recorder: None,
    };
    // Drop without finalize: the fallback must release the output session
    // before the recorder finalization and the device release.
    drop(guard);
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < close,
        "fallback order: {timeline:?}"
    );
}

// ---------------------------------------------------------------------------
// The full gesture pipeline (M10_TASK.md §9)
// ---------------------------------------------------------------------------

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: touchpad_core::LogicalPixels::try_new(dx).unwrap(),
        dy: touchpad_core::LogicalPixels::try_new(dy).unwrap(),
    }
}

fn down_left() -> OutputEvent {
    OutputEvent::ButtonDown(touchpad_core::MouseButton::Left)
}

fn up_left() -> OutputEvent {
    OutputEvent::ButtonUp(touchpad_core::MouseButton::Left)
}

fn down_right() -> OutputEvent {
    OutputEvent::ButtonDown(touchpad_core::MouseButton::Right)
}

fn up_right() -> OutputEvent {
    OutputEvent::ButtonUp(touchpad_core::MouseButton::Right)
}

fn scroll_delta(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::ScrollDelta {
        dx: touchpad_core::LogicalPixels::try_new(dx).unwrap(),
        dy: touchpad_core::LogicalPixels::try_new(dy).unwrap(),
    }
}

/// All seven M10 gestures — pointer, physical Left drag, tap-to-click,
/// tap-and-drag with drag lock, 2D natural scroll, secondary tap, and
/// buttonpad two-finger physical click — travel through
/// decoder → arbiter → output in order, with no raw contact leakage (the
/// output receives only resolved semantic events).
#[test]
fn gesture_pipeline_decoder_arbiter_output_without_raw_leakage() {
    let mut device = mock_touchpad();

    // Build the full raw stream, then split it into read chunks of at most
    // 60 events (the runtime's read buffer holds 64 events; a smaller chunk
    // leaves headroom and keeps the chunk count exact).
    let mut all: Vec<u8> = Vec::new();

    // 1. One-finger pointer: begin (1,1)mm → (2,1)mm commits a 10 px move;
    //    → (3,1)mm adds 10 px; end.
    all.extend(one_frame(1, 1000, 10, 0, 100, 100, false));
    all.extend(one_frame(1, 1100, 10, 0, 200, 100, false));
    all.extend(one_frame(1, 1200, 10, 0, 300, 100, false));
    all.extend(end_frame(1, 1300, 0, false));

    // 2. Physical Left drag: press, move, move, release.
    all.extend(one_frame(2, 1000, 11, 0, 100, 100, true));
    all.extend(one_frame(2, 1100, 11, 0, 200, 100, true));
    all.extend(one_frame(2, 1200, 11, 0, 300, 100, true));
    all.extend(one_frame(2, 1300, 11, 0, 300, 100, false));
    all.extend(end_frame(2, 1400, 0, false));

    // 3. Tap-to-click: begin + end within 180 ms and 3 mm.
    all.extend(one_frame(3, 1000, 12, 0, 100, 100, false));
    all.extend(end_frame(3, 1100, 0, false));

    // 4. Tap-and-drag with sticky drag lock: tap (opens the follow-up
    //    window), a new contact within the 350 ms gap stays pending until its
    //    pointer motion commits; that commit presses synthetically before the
    //    first drag delta, then lift → sticky lock holds left.
    all.extend(one_frame(4, 1000, 13, 0, 100, 100, false));
    all.extend(end_frame(4, 1100, 0, false));
    all.extend(one_frame(4, 1300, 14, 0, 100, 100, false));
    all.extend(one_frame(4, 1400, 14, 0, 200, 100, false));
    all.extend(one_frame(4, 1500, 14, 0, 300, 100, false));
    all.extend(end_frame(4, 1600, 0, false));

    // 5. Two-finger 2D natural scroll: two fingers move diagonally +1 mm per
    //    step; the scroll commits on the first delta and ends on finger loss.
    all.extend(one_frame(5, 1000, 15, 0, 100, 100, false));
    all.extend(two_frame(5, 1100, 15, 100, 100, 16, 200, 100, false));
    all.extend(two_frame(5, 1200, 15, 200, 200, 16, 300, 200, false));
    all.extend(two_frame(5, 1300, 15, 300, 300, 16, 400, 300, false));
    // Finger 16 (slot 1) ends with a clean Ended record while finger 15
    // continues: dropping below two ends the scroll on this frame.
    all.extend(end_frame(5, 1400, 1, false));
    // Finger 15 (slot 0) ends.
    all.extend(end_frame(5, 1500, 0, false));

    // 6. Secondary tap: two fingers tap quickly; the first finger ends with
    //    a clean Ended record (M9 review R6) while the second continues, then
    //    the second ends.
    all.extend(one_frame(6, 1000, 17, 0, 100, 100, false));
    all.extend(two_frame(6, 1100, 17, 100, 100, 18, 200, 100, false));
    // Frame: slot 0 tid 17 Ended, slot 1 tid 18 Active.
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_SLOT, 0));
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_TRACKING_ID, -1));
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_SLOT, 1));
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_TRACKING_ID, 18));
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_POSITION_X, 200));
    all.extend(ev(6, 1200, EV_ABS, ABS_MT_POSITION_Y, 100));
    all.extend(syn(6, 1200));
    all.extend(end_frame(6, 1300, 1, false));

    // 7. Buttonpad two-finger physical click: left press while exactly two
    //    fingers are down is latched to the secondary (right) button.
    all.extend(one_frame(7, 1000, 19, 0, 100, 100, false));
    all.extend(two_frame(7, 1100, 19, 100, 100, 20, 200, 100, false));
    all.extend(two_frame(7, 1200, 19, 100, 100, 20, 200, 100, true));
    all.extend(two_frame(7, 1300, 19, 100, 100, 20, 200, 100, false));
    all.extend(end_frame(7, 1400, 0, false));
    all.extend(end_frame(7, 1500, 1, false));

    const CHUNK_EVENTS: usize = 60;
    let chunk_bytes = CHUNK_EVENTS * 24;
    let n_chunks = all.len().div_ceil(chunk_bytes);
    for chunk in all.chunks(chunk_bytes) {
        device.push_raw(chunk.to_vec());
    }

    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    h.with_readiness(vec![true; n_chunks]);
    let mut env = h.env();
    let result = run_takeover(&mut env, 60);
    // The session runs until the deadline (all events processed, then idle);
    // the exit is clean only when the final lock release also succeeded.
    assert!(result.is_ok(), "{result:?}");
    drop(env);

    let submitted = h.streaming_state.borrow().submitted.clone();
    let expected: Vec<OutputEvent> = vec![
        // 1. Pointer: two 10 px moves (the first includes the commit).
        move_event(10.0, 0.0),
        move_event(10.0, 0.0),
        // 2. Physical Left drag: down, two moves, up.
        down_left(),
        move_event(10.0, 0.0),
        move_event(10.0, 0.0),
        up_left(),
        // 3. Tap: one click pair.
        down_left(),
        up_left(),
        // 4. Tap-and-drag: first tap's click pair, then the committed
        //    follow-up emits synthetic press before two drag moves; the lift
        //    engages the sticky lock (no up yet).
        down_left(),
        up_left(),
        down_left(),
        move_event(10.0, 0.0),
        move_event(10.0, 0.0),
        // 5. The two-finger candidate releases the sticky lock (aggregate
        //    rules), then the 2D natural scroll lifecycle.
        up_left(),
        OutputEvent::ScrollBegin,
        scroll_delta(10.0, 10.0),
        scroll_delta(10.0, 10.0),
        OutputEvent::ScrollEnd,
        // 6. Secondary tap: one right click pair.
        down_right(),
        up_right(),
        // 7. Buttonpad two-finger physical click: one latched right pair.
        down_right(),
        up_right(),
    ];
    assert_eq!(submitted, expected, "the full semantic stream must match");
    // No raw contact data can leak: every submitted event is a resolved
    // semantic event by construction; assert the exact event kinds.
    for event in &submitted {
        assert!(
            matches!(
                event,
                OutputEvent::PointerMove { .. }
                    | OutputEvent::ButtonDown(_)
                    | OutputEvent::ButtonUp(_)
                    | OutputEvent::ScrollBegin
                    | OutputEvent::ScrollDelta { .. }
                    | OutputEvent::ScrollEnd
            ),
            "raw contact leakage: {event:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// M11: pure profile routing, experimental banner, and the fake-backed
// command path (M11_TASK.md §11/§12)
// ---------------------------------------------------------------------------

/// `select_profile("m10-linear-v1")` constructs **exactly** the M10
/// profile's arbiter configuration with the fidelity stage disabled
/// (M11_TASK.md §5: the M10 path must stay output-compatible and never pass
/// committed pointer motion through M11 fidelity logic).
#[test]
fn select_profile_m10_constructs_exact_m10_config_fidelity_disabled() {
    let selected = select_profile("m10-linear-v1").expect("m10 profile selects");
    let m10 = M10Profile::new().expect("documented constants validate");
    assert_eq!(selected.name, M10Profile::NAME);
    assert_eq!(selected.name, "m10-linear-v1");
    assert_eq!(selected.arbiter_config, m10.arbiter_config());
    assert!(!selected.arbiter_config.is_fidelity_enabled());
    assert!(selected.arbiter_config.fidelity_config().is_none());
    // Every inherited M7–M9 value is the exact M10 value.
    assert_eq!(
        selected.arbiter_config.motion_threshold_mm(),
        m10.motion_threshold_mm()
    );
    assert_eq!(
        selected.arbiter_config.logical_pixels_per_mm(),
        m10.logical_pixels_per_mm()
    );
    assert_eq!(
        selected.arbiter_config.tap_config(),
        m10.arbiter_config().tap_config()
    );
    assert_eq!(
        selected.arbiter_config.two_finger_config(),
        m10.arbiter_config().two_finger_config()
    );
    assert_eq!(selected.description, M10_PROFILE_DESCRIPTION);
    // The M10 banner names the baseline profile without any experimental
    // claim (the M11 experimental banner is M11-only).
    assert!(
        selected.banner.contains("m10-linear-v1"),
        "{}",
        selected.banner
    );
    assert!(
        !selected.banner.contains("EXPERIMENTAL"),
        "the M10 banner must not carry the M11 experimental claim: {}",
        selected.banner
    );
}

/// `select_profile("m11-fidelity-v1")` constructs **exactly** the M11
/// profile's arbiter configuration — the inherited M10/M7–M9 config plus the
/// M11 fidelity stage enabled (M11_TASK.md §5: `M11Profile` obtains the
/// M7–M9 config from `M10Profile` and only adds fidelity).
#[test]
fn select_profile_m11_constructs_exact_m11_config_fidelity_enabled() {
    let selected = select_profile("m11-fidelity-v1").expect("m11 profile selects");
    let m11 = M11Profile::new().expect("documented constants validate");
    assert_eq!(selected.name, M11Profile::NAME);
    assert_eq!(selected.name, "m11-fidelity-v1");
    assert_eq!(selected.arbiter_config, m11.arbiter_config());
    assert!(selected.arbiter_config.is_fidelity_enabled());
    assert_eq!(
        selected.arbiter_config.fidelity_config(),
        m11.arbiter_config().fidelity_config()
    );
    // The M10 base values are inherited exactly (only fidelity is added).
    let m10 = M10Profile::new().expect("documented constants validate");
    assert_eq!(
        selected.arbiter_config.motion_threshold_mm(),
        m10.motion_threshold_mm()
    );
    assert_eq!(
        selected.arbiter_config.logical_pixels_per_mm(),
        m10.logical_pixels_per_mm()
    );
    assert_eq!(
        selected.arbiter_config.tap_config(),
        m10.arbiter_config().tap_config()
    );
    assert_eq!(
        selected.arbiter_config.two_finger_config(),
        m10.arbiter_config().two_finger_config()
    );
    assert_eq!(selected.description, M11_PROFILE_DESCRIPTION);
}

#[test]
fn select_profile_m12_constructs_exact_m12_config_and_banner() {
    let selected = select_profile("m12-scroll-v1").expect("m12 profile selects");
    let m12 = M12Profile::new().expect("documented constants validate");
    assert_eq!(selected.name, M12Profile::NAME);
    assert_eq!(selected.arbiter_config, m12.arbiter_config());
    assert!(selected.arbiter_config.is_fidelity_enabled());
    assert!(selected.arbiter_config.is_scroll_fidelity_enabled());
    assert_eq!(selected.description, M12_PROFILE_DESCRIPTION);
    let banner = &selected.banner;
    for required in [
        "m12-scroll-v1",
        "EXPERIMENTAL",
        "UNCALIBRATED",
        "NOT the default",
        "macOS-equivalence",
        "NO live M12 validation",
        "1..=300",
    ] {
        assert!(banner.contains(required), "missing {required:?}: {banner}");
    }
}

#[test]
fn select_profile_m17_requires_explicit_feel_and_default_matches_m16() {
    assert_eq!(
        select_profile("m17-tunable-v1"),
        Err(ProfileSelectionError::MissingFeelConfig)
    );
    let selected = select_profile_with_feel("m17-tunable-v1", Some(FeelConfig::default())).unwrap();
    assert_eq!(selected.name, M17Profile::NAME);
    assert_eq!(
        selected.arbiter_config,
        M16Profile::new().unwrap().arbiter_config()
    );
    assert!(selected.banner.contains("m17-tunable-v1"));
    assert!(selected.banner.contains("FeelConfig"));
    assert!(selected.banner.contains("live-unqualified"));
}

#[test]
fn select_profile_m17_applies_tuning_but_cannot_mutate_earlier_profile() {
    let mut feel = FeelConfig::default();
    feel.set_key("pointer.tracking_speed", "1.5").unwrap();
    let selected = select_profile_with_feel("m17-tunable-v1", Some(feel.clone())).unwrap();
    let expected = M17Profile::with_feel(feel)
        .unwrap()
        .arbiter_config()
        .unwrap();
    assert_eq!(selected.arbiter_config, expected);
    assert_eq!(
        select_profile_with_feel("m16-production-v1", Some(FeelConfig::default())),
        Err(ProfileSelectionError::UnexpectedFeelConfig)
    );
}

#[test]
fn select_profile_m18_requires_settings_and_installs_gesture_bindings() {
    assert_eq!(
        select_profile("m18-remap-v1"),
        Err(ProfileSelectionError::MissingSettings)
    );
    let settings = UserSettings::macos_inspired();
    let selected = select_profile_with_settings("m18-remap-v1", Some(settings.clone())).unwrap();
    let expected = M18Profile::new(settings).unwrap().arbiter_config().unwrap();
    assert_eq!(selected.name, M18Profile::NAME);
    assert_eq!(selected.arbiter_config, expected);
    assert!(selected.arbiter_config.is_gesture_bindings_enabled());
    assert!(selected.banner.contains("m18-remap-v1"));
    assert!(selected
        .banner
        .contains("arbitrary shell commands are not supported"));
}

#[test]
fn select_profile_m19_uses_settings_policy_with_live_reload_banner() {
    let settings = UserSettings::default();
    let selected = select_profile_with_settings("m19-live-v1", Some(settings.clone())).unwrap();
    let expected = M19Profile::new(settings).unwrap().arbiter_config().unwrap();
    assert_eq!(selected.name, M19Profile::NAME);
    assert_eq!(selected.arbiter_config, expected);
    assert!(selected.banner.contains("m19-live-v1"));
    assert!(selected.banner.contains("last-good"));
    assert!(selected.banner.contains("neutral interaction boundary"));
}

/// The M11 banner states every claim M11_TASK.md §11 requires: experimental
/// and uncalibrated; not the default; no macOS equivalence claim; no live
/// M11 validation has occurred; and the M10 safety opt-ins (`--takeover`,
/// `--confirm TAKEOVER`, `--output-qualified`) plus the `1..=300` second
/// maximum-duration bound still apply.
#[test]
fn m11_banner_contains_all_required_claims() {
    let selected = select_profile("m11-fidelity-v1").expect("m11 profile selects");
    let banner = &selected.banner;
    assert!(banner.contains("m11-fidelity-v1"), "{banner}");
    assert!(banner.contains("EXPERIMENTAL"), "{banner}");
    assert!(banner.contains("UNCALIBRATED"), "{banner}");
    assert!(banner.contains("NOT the default"), "{banner}");
    assert!(banner.contains("macOS-equivalence"), "{banner}");
    assert!(banner.contains("NO live M11 validation"), "{banner}");
    // M10 safety opt-ins and the duration bound still apply.
    assert!(banner.contains("--takeover"), "{banner}");
    assert!(banner.contains("--confirm TAKEOVER"), "{banner}");
    assert!(banner.contains("--output-qualified"), "{banner}");
    assert!(banner.contains("1..=300"), "{banner}");
}

/// An unknown profile fails in the pure helper with the current accepted set
/// named. The helper
/// is pure — it constructs no device, output session, recorder, countdown,
/// or grab object on any path, so the failure has no side effects.
#[test]
fn select_profile_unknown_fails_without_side_effects() {
    let error = match select_profile("macos-like") {
        Err(error @ ProfileSelectionError::Unknown { .. }) => error,
        other => panic!("expected Unknown, got {other:?}"),
    };
    match &error {
        ProfileSelectionError::Unknown { found } => assert_eq!(found, "macos-like"),
        other => panic!("expected Unknown, got {other:?}"),
    }
    let text = error.to_string();
    assert!(text.contains("m10-linear-v1"), "{text}");
    assert!(text.contains("m11-fidelity-v1"), "{text}");
    assert!(text.contains("m12-scroll-v1"), "{text}");
}

/// The fake-backed takeover command path with `m11-fidelity-v1`: the
/// experimental banner is written **before** the step-6 device status line —
/// which is only printed after the device open, the output prepare, and the
/// recorder attach, so the banner precedes every device/output/recorder/
/// countdown/grab side effect — the fidelity-enabled config routes the
/// committed pointer motion through the pipeline to the fake output, and
/// the deadline stop is clean with the ordered cleanup. No real
/// device/portal/libei/desktop input is involved (M11_TASK.md §1).
#[test]
fn m11_takeover_command_path_banner_before_side_effects_and_clean_deadline() {
    let mut device = mock_touchpad();
    // One committed pointer move: begin (1,1)mm → (2,1)mm.
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false));
    batch.extend(one_frame(1, 1100, 10, 0, 200, 100, false));
    batch.extend(end_frame(1, 1200, 0, false));
    device.push_raw(batch);

    let mut h = Harness::happy();
    h.with_device(device);
    h.with_marker_recorder();
    // First poll ready (the events), then idle (the deadline expires).
    h.with_readiness(vec![true]);

    let mut env = h.env();
    let result = super::run(
        &mut env,
        &device_path(),
        &temp_trace("m11"),
        1,
        "m11-fidelity-v1",
        ProfileInputs::default(),
    );
    assert!(result.is_ok(), "{result:?}");
    drop(env);

    let err_text = String::from_utf8(h.err).unwrap();
    // The banner is written before the step-6 device status line (printed
    // only after open/prepare/recorder attach) and before the countdown and
    // the stop report.
    let banner = err_text
        .find("m11-fidelity-v1 is EXPERIMENTAL")
        .expect("the M11 banner is written");
    let device_line = err_text
        .find("device: /dev/input/event0")
        .expect("the device status line is written");
    let countdown = err_text
        .find("takeover in 3 second(s)")
        .expect("the countdown runs");
    let stopped = err_text
        .find("takeover stopped")
        .expect("the stop report is written");
    assert!(
        banner < device_line && device_line < countdown && countdown < stopped,
        "banner ordering: {err_text}"
    );
    // The fidelity-enabled pipeline emitted the committed move to the fake
    // output (the M11 stage ran, not a skipped/disabled branch).
    let submitted = h.streaming_state.borrow().submitted.clone();
    assert_eq!(submitted, vec![move_event(10.0, 0.0)], "{submitted:?}");
    // Ordered cleanup: output release → recorder finish → ungrab → close,
    // exactly one grab and one ungrab.
    let timeline = h.timeline.borrow();
    let release = pos(&timeline, "output:release_all");
    let finish = pos(&timeline, "recorder:finish");
    let ungrab = pos(&timeline, ", false)");
    let close = pos(&timeline, "close(");
    assert!(
        release < finish && finish < ungrab && ungrab < close,
        "cleanup order: {timeline:?}"
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, true))),
        1
    );
    assert_eq!(
        h.sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
}

// ---------------------------------------------------------------------------
// Trace / replay parity (M10_TASK.md §9)
// ---------------------------------------------------------------------------

/// The same raw input used by the takeover replays through the same decoder
/// to the same frames: the trace is the ground truth of the takeover.
#[test]
fn takeover_trace_replays_to_the_same_frames() {
    let mut device = mock_touchpad();
    let mut batch = Vec::new();
    batch.extend(one_frame(1, 1000, 10, 0, 100, 100, false));
    batch.extend(one_frame(1, 1100, 10, 0, 200, 100, false));
    batch.extend(end_frame(1, 1200, 0, false));
    device.push_raw(batch);
    let trace_path = temp_trace("parity");

    let mut h = Harness::happy();
    h.with_device(device); // real TraceRecorder
    h.with_readiness(vec![true]);
    let mut env = h.env();
    // Run the takeover with the trace written to a real file.
    let result = super::run(
        &mut env,
        &device_path(),
        &trace_path,
        1,
        "m10-linear-v1",
        ProfileInputs::default(),
    );
    assert!(result.is_ok(), "{result:?}");
    drop(env);

    // Replay the trace through the same Type-B decoder used live.
    let mut decoder = touchpad_linux::TypeBDecoder::new(touchpad_linux::RecordingFrameSink::new());
    touchpad_trace::ReplayDriver::replay(std::fs::File::open(&trace_path).unwrap(), &mut decoder)
        .expect("the takeover trace must replay cleanly");
    let frames = decoder.into_sink().take_frames();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].contacts[0].tracking_id, 10);
    assert_eq!(
        frames[0].contacts[0].state,
        touchpad_core::ContactState::Began
    );
    // The live path processed the same three frames.
    let st = h.streaming_state.borrow();
    assert!(
        !st.submitted.is_empty(),
        "the takeover emitted pointer output"
    );
    std::fs::remove_file(&trace_path).ok();
}

fn temp_trace(tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "touchpadctl-takeover-{}-{}-{}.jsonl",
        std::process::id(),
        unique,
        tag
    ))
}
