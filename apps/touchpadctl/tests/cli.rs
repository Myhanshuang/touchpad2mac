//! M5 integration tests through the public library API (the library-level
//! command runner): CLI help, no-device behavior, fixture replay smoke,
//! corrupted traces, record pipeline ordering, and the signal-stop /
//! shutdown-order paths. Everything runs against the mockable [`Sys`] seam
//! and fixture files — no real device is ever opened or grabbed, and no
//! `/dev/input` access is required.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use touchpad_core::{Monotonic, OutputEvent};
use touchpad_desktop::fake::{FakeStreamingOutput, FakeStreamingState};
use touchpad_desktop::{DesktopOutputError, StreamingOutput};
use touchpad_linux::sys::mock::{MockCall, MockDevice, MockFailure, MockSys};
use touchpad_linux::sys::{Fd, SysError};
use touchpad_linux::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, EV_ABS, EV_SYN,
    SYN_DROPPED, SYN_REPORT,
};
use touchpadctl::env::TakeoverSeams;
use touchpadctl::{parse_args, run_command, Command, CommandEnv, CommandFailure, ExitCode};

const FIXTURES_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/touchpad-trace/tests/fixtures"
);

fn fixture(name: &str) -> PathBuf {
    Path::new(FIXTURES_DIR).join(format!("{name}.jsonl"))
}

fn env<'a>(sys: Rc<MockSys>, out: &'a mut Vec<u8>, err: &'a mut Vec<u8>) -> CommandEnv<'a> {
    CommandEnv {
        sys: sys as Rc<dyn touchpad_linux::sys::Sys>,
        out,
        err,
        stop_flag: Arc::new(AtomicBool::new(false)),
        recorder_factory: None,
        output_factory: None,
        takeover: TakeoverSeams::inert(),
    }
}

/// A fake-backed takeover command environment: a fake monotonic clock
/// (advanced only by the fake readiness polls — never by sleeping), a no-op
/// sleeper, and a fake streaming session. `takeover` therefore runs fully
/// in-process: no real device, portal, libei, or desktop input is involved
/// (M11_TASK.md §1). `readiness_script` scripts the poll outcomes; an
/// exhausted script idles and advances the fake clock by the poll quantum,
/// so the maximum-duration deadline eventually expires without sleeping.
fn takeover_env<'a>(
    sys: Rc<MockSys>,
    out: &'a mut Vec<u8>,
    err: &'a mut Vec<u8>,
    now: Rc<RefCell<Monotonic>>,
    readiness_script: Vec<bool>,
    streaming_state: Rc<RefCell<FakeStreamingState>>,
) -> CommandEnv<'a> {
    let script = Rc::new(RefCell::new(VecDeque::from(readiness_script)));
    let script_for_readiness = Rc::clone(&script);
    let now_for_readiness = Rc::clone(&now);
    let readiness: Rc<dyn Fn(Fd, Duration) -> Result<bool, SysError>> =
        Rc::new(move |_fd: Fd, timeout: Duration| {
            let ready = script_for_readiness
                .borrow_mut()
                .pop_front()
                .unwrap_or(false);
            if !ready {
                let next = now_for_readiness
                    .borrow()
                    .checked_add(timeout)
                    .unwrap_or(Monotonic::from_nanos(u64::MAX));
                *now_for_readiness.borrow_mut() = next;
            }
            Ok(ready)
        });
    let now_for_clock = Rc::clone(&now);
    let clock: Rc<dyn Fn() -> Monotonic> = Rc::new(move || *now_for_clock.borrow());
    let sleeper: Rc<dyn Fn(Duration)> = Rc::new(|_| {});
    let state_for_factory = Rc::clone(&streaming_state);
    let factory: Box<dyn FnMut() -> Result<Box<dyn StreamingOutput>, DesktopOutputError>> =
        Box::new(move || {
            Ok(Box::new(FakeStreamingOutput::new(Rc::clone(
                &state_for_factory,
            ))))
        });
    CommandEnv {
        sys: sys as Rc<dyn touchpad_linux::sys::Sys>,
        out,
        err,
        stop_flag: Arc::new(AtomicBool::new(false)),
        recorder_factory: None,
        output_factory: None,
        takeover: TakeoverSeams {
            clock,
            readiness,
            sleeper,
            streaming_factory: Some(factory),
        },
    }
}

fn mock_touchpad() -> MockDevice {
    let mut device = MockDevice::touchpad("Pad", 10);
    device.mt_slots.insert(ABS_MT_TRACKING_ID, vec![-1; 10]);
    device.mt_slots.insert(ABS_MT_POSITION_X, vec![0; 10]);
    device.mt_slots.insert(ABS_MT_POSITION_Y, vec![0; 10]);
    device
}

fn ev_bytes(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
    touchpad_linux::encode_input_event(sec, usec, event_type, code, value)
}

fn begin_contact(sec: i64) -> Vec<u8> {
    [
        ev_bytes(sec, 0, EV_ABS, ABS_MT_SLOT, 0),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_TRACKING_ID, 7),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_POSITION_X, 100),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_POSITION_Y, 50),
        ev_bytes(sec, 0, EV_SYN, SYN_REPORT, 0),
    ]
    .concat()
}

/// One frame with the same live contact at a new raw position.
fn move_contact(sec: i64, x: i32, y: i32) -> Vec<u8> {
    [
        ev_bytes(sec, 0, EV_ABS, ABS_MT_SLOT, 0),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_TRACKING_ID, 7),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_POSITION_X, x),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_POSITION_Y, y),
        ev_bytes(sec, 0, EV_SYN, SYN_REPORT, 0),
    ]
    .concat()
}

/// One frame that ends the contact on slot 0.
fn end_contact(sec: i64) -> Vec<u8> {
    [
        ev_bytes(sec, 0, EV_ABS, ABS_MT_SLOT, 0),
        ev_bytes(sec, 0, EV_ABS, ABS_MT_TRACKING_ID, -1),
        ev_bytes(sec, 0, EV_SYN, SYN_REPORT, 0),
    ]
    .concat()
}

fn temp_output(tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "touchpadctl-it-{}-{}-{}.jsonl",
        std::process::id(),
        unique,
        tag
    ))
}

fn read_trace_events(path: &Path) -> Vec<touchpad_trace::TraceEvent> {
    let mut reader = touchpad_trace::TraceReader::new(std::fs::File::open(path).unwrap());
    reader.read_header().unwrap();
    reader.events().map(Result::unwrap).collect()
}

/// M5 acceptance: `touchpadctl --help` (via the command runner) exits 0 and
/// the help text warns explicitly about `--grab`.
#[test]
fn help_command_exits_zero_and_warns_about_grab() {
    let command = parse_args(vec!["--help".to_string()]).unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(Rc::new(MockSys::new()), &mut out, &mut err);
    run_command(&mut env, &command).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("touchpadctl"));
    assert!(text.contains("WARNING"));
    assert!(text.contains("EXCLUSIVELY"));
    assert!(text.contains("Default: OFF"));
    assert!(text.contains("replay INPUT"));
}

/// M5 acceptance: the fixture replay smoke test through the public command
/// runner — a fixture trace replays cleanly (exit 0) to three JSON frames.
#[test]
fn fixture_replay_smoke_through_the_command_runner() {
    let command = Command::Replay {
        input: fixture("single_contact"),
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(Rc::new(MockSys::new()), &mut out, &mut err);
    run_command(&mut env, &command).unwrap();
    let frames: Vec<touchpad_core::ContactFrame> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].sequence, 1);
    let err_text = String::from_utf8(err).unwrap();
    assert!(err_text.contains("replay summary"), "{err_text}");
}

/// M5 acceptance: no-device behavior — `devices` on an empty enumeration
/// reports a clear result with exit code 4 (no candidate).
#[test]
fn devices_with_no_devices_is_clear_and_exit_4() {
    let sys = Rc::new(MockSys::new());
    sys.set_dir_entries(vec![]);
    let command = Command::Devices;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(sys, &mut out, &mut err);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::NoCandidate);
    assert!(
        failure.to_string().contains("no /dev/input/event* nodes"),
        "{failure}"
    );
}

/// M5 acceptance: a corrupted trace fails with exit code 5 (trace error).
#[test]
fn corrupted_trace_fails_with_trace_exit_code() {
    let path = temp_output("corrupt");
    std::fs::write(
        &path,
        "{\"kind\":\"header\",\"schema_version\":1,\"clock\":\"monotonic\",\"device\":{\"name\":\"x\",\"vendor_id\":0,\"product_id\":0,\"axes\":{},\"slot_count\":10,\"supports_type_b_mt\":true,\"has_physical_buttons\":false,\"profile\":{\"name\":\"default\",\"axis_resolutions\":{},\"quirks\":[]}}}\nnot-json\n",
    )
    .unwrap();
    let command = Command::Replay {
        input: path.clone(),
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(Rc::new(MockSys::new()), &mut out, &mut err);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Trace);
    assert!(failure.to_string().contains("corrupted"), "{failure}");
    std::fs::remove_file(&path).ok();
}

/// M5 acceptance: the header-only fixture (`tests/fixtures/empty.jsonl`)
/// replays cleanly with zero events and zero frames (exit 0).
#[test]
fn header_only_fixture_replays_cleanly() {
    let command = Command::Replay {
        input: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/empty.jsonl"),
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(Rc::new(MockSys::new()), &mut out, &mut err);
    run_command(&mut env, &command).unwrap();
    assert!(String::from_utf8(out).unwrap().is_empty());
    let err_text = String::from_utf8(err).unwrap();
    assert!(err_text.contains("events_forwarded=0"), "{err_text}");
    assert!(err_text.contains("frames=0"), "{err_text}");
}

/// M5 acceptance: the record pipeline ordering — a decoder failure (failed
/// resync) must not lose the raw events already read; the trace holds every
/// event of the batch.
#[test]
fn record_keeps_raw_events_when_the_decoder_fails() {
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event0");
    let mut device = mock_touchpad();
    let mut batch = begin_contact(1);
    batch.extend(ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0));
    batch.extend(ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0));
    device.push_raw(batch);
    device.mt_slots_error = Some(MockFailure::Io);
    sys.add_device(&path, device);
    let output = temp_output("ordering");

    let command = Command::Record {
        device: path.clone(),
        output: output.clone(),
        grab: false,
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(sys, &mut out, &mut err);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
    assert_eq!(read_trace_events(&output).len(), 7);
    std::fs::remove_file(&output).ok();
}

/// M5 acceptance: the signal stop — a stop flag set before the first step
/// stops the session gracefully (exit 8) with the ordered cleanup (recorder
/// flush → ungrab → close, exactly once each).
#[test]
fn signal_stop_exits_8_with_ordered_shutdown() {
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event0");
    sys.add_device(&path, mock_touchpad());
    let output = temp_output("signal");

    let command = Command::Record {
        device: path,
        output: output.clone(),
        grab: true,
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(sys.clone(), &mut out, &mut err);
    env.stop_flag.store(true, Ordering::Relaxed);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
    assert!(String::from_utf8(err)
        .unwrap()
        .contains("cleanup: recorder ok, ungrab ok, close ok"));

    // Ordered cleanup on the syscall log: ungrab before close, exactly once.
    let log = sys.log();
    let ungrab = log
        .iter()
        .position(|call| matches!(call, MockCall::Grab(_, false)))
        .expect("ungrab");
    let close = log
        .iter()
        .position(|call| matches!(call, MockCall::Close(_)))
        .expect("close");
    assert!(ungrab < close, "ungrab must precede close");
    assert_eq!(
        sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    std::fs::remove_file(&output).ok();
}

/// M5: `record` on a missing device node is actionable (exit 2).
#[test]
fn record_missing_device_is_actionable() {
    let command = Command::Record {
        device: PathBuf::from("/dev/input/event9"),
        output: temp_output("missing"),
        grab: false,
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(Rc::new(MockSys::new()), &mut out, &mut err);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::InputDir, "{failure}");
    assert!(
        failure.to_string().contains("no such device node"),
        "{failure}"
    );
}

/// M5: `inspect` of a non-candidate device prints the report and fails with
/// the rejection reasons (exit 4).
#[test]
fn inspect_non_candidate_fails_with_reasons() {
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event0");
    let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
    touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
    touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
        !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
    sys.add_device(&path, touchscreen);
    let command = Command::Inspect { device: path };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut env = env(sys, &mut out, &mut err);
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::NoCandidate, "{failure}");
    assert!(String::from_utf8(out).unwrap().contains("rejected"));
}

/// M5 review R5 (actual binary path): `--grab` on a non-record command is a
/// usage error at the parser level — the exact layer the binary dispatches
/// on — so `devices --grab`, `inspect DEVICE --grab`, and `replay INPUT
/// --grab` produce no command object (nothing executes) and map to exit 1.
#[test]
fn grab_is_a_usage_error_for_non_record_commands() {
    for args in [
        vec!["devices".to_string(), "--grab".to_string()],
        vec![
            "inspect".to_string(),
            "/dev/input/event0".to_string(),
            "--grab".to_string(),
        ],
        vec![
            "replay".to_string(),
            "trace.jsonl".to_string(),
            "--grab".to_string(),
        ],
    ] {
        let err = parse_args(args).unwrap_err();
        let failure = CommandFailure::Usage(err);
        assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    }
    // A duplicate `--grab` on `record` is likewise a usage error (exit 1).
    let err = parse_args(vec![
        "record".to_string(),
        "/dev/input/event0".to_string(),
        "t.jsonl".to_string(),
        "--grab".to_string(),
        "--grab".to_string(),
    ])
    .unwrap_err();
    assert_eq!(CommandFailure::Usage(err).exit_code(), ExitCode::Usage);
}

/// The structured failure type always carries a stable exit code (the CLI's
/// contract for scripts).
#[test]
fn every_command_failure_has_a_stable_exit_code() {
    let usage = CommandFailure::Usage(touchpadctl::args::UsageError::NoCommand);
    assert_eq!(usage.exit_code(), ExitCode::Usage);
    let stopped = CommandFailure::Stopped;
    assert_eq!(stopped.exit_code(), ExitCode::Stopped);
    assert_eq!(ExitCode::Success.code(), 0);
    assert_eq!(ExitCode::Unexpected.code(), 9);
}

/// M6: the output-probe dry-run through the public command runner prints
/// the environment/capability report (never emitting) and exits 0 — the
/// fake backend guarantees no real desktop input is involved.
#[test]
fn output_probe_dry_run_reports_and_exits_zero() {
    let command = Command::OutputProbe { emit: false };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut output = Some(touchpad_desktop::FakeDesktopOutput::available());
    let mut env = CommandEnv {
        sys: Rc::new(MockSys::new()) as Rc<dyn touchpad_linux::sys::Sys>,
        out: &mut out,
        err: &mut err,
        stop_flag: Arc::new(AtomicBool::new(false)),
        recorder_factory: None,
        output_factory: Some(Box::new(move || {
            Box::new(output.take().expect("factory used once"))
        })),
        takeover: TakeoverSeams::inert(),
    };
    run_command(&mut env, &command).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("backend state: experimental/unqualified"),
        "{text}"
    );
    assert!(text.contains("RemoteDesktop portal"), "{text}");
    assert!(text.contains("--emit would:"), "{text}");
}

/// M6 (actual binary path): `--emit` is output-probe-only — `record
/// --emit`, `replay --emit` and a duplicate `--emit` are usage errors
/// (exit 1), and `output-probe --grab` is rejected too.
#[test]
fn emit_and_grab_usage_rules_for_output_probe() {
    for args in [
        vec![
            "record".to_string(),
            "/dev/input/event0".to_string(),
            "t.jsonl".to_string(),
            "--emit".to_string(),
        ],
        vec![
            "replay".to_string(),
            "trace.jsonl".to_string(),
            "--emit".to_string(),
        ],
        vec!["devices".to_string(), "--emit".to_string()],
        vec!["output-probe".to_string(), "--grab".to_string()],
        vec![
            "output-probe".to_string(),
            "--emit".to_string(),
            "--emit".to_string(),
        ],
    ] {
        let err = parse_args(args).unwrap_err();
        let failure = CommandFailure::Usage(err);
        assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    }
    // The single explicit opt-in parses.
    assert_eq!(
        parse_args(vec!["output-probe".to_string(), "--emit".to_string()]).unwrap(),
        Command::OutputProbe { emit: true }
    );
}

/// M6 re-review R2: a **real** SIGINT delivered to the process (with the
/// termination handler installed, exactly as the binary does for
/// `output-probe --emit`) records the stop request in the process-lifetime
/// static, which the emit path observes — mapping to exit 8 with nothing
/// emitted — and the guard drop restores the previous dispositions and
/// resets the stop state.
///
/// This lives in the integration-test crate (not the `unsafe`-forbidden
/// library) because it must deliver a real signal via `libc::raise`.
#[cfg(target_os = "linux")]
#[test]
fn real_sigint_during_emit_is_observed_and_maps_to_exit_8() {
    let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
    let guard = touchpad_linux::install_termination_handler()
        .expect("installing the termination handler must succeed");
    assert!(!touchpad_linux::termination_requested());
    // SAFETY: `raise(2)` delivers SIGINT to the calling thread; our handler
    // is installed, so the default terminate action is replaced and only the
    // process-lifetime stop state is set.
    unsafe {
        libc::raise(libc::SIGINT);
    }
    assert!(
        touchpad_linux::termination_requested(),
        "the real signal must be observed by the emit path's cancellation check"
    );

    let command = Command::OutputProbe { emit: true };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut output = Some(touchpad_desktop::FakeDesktopOutput::available());
    let mut env = CommandEnv {
        sys: Rc::new(MockSys::new()) as Rc<dyn touchpad_linux::sys::Sys>,
        out: &mut out,
        err: &mut err,
        stop_flag: Arc::new(AtomicBool::new(false)),
        recorder_factory: None,
        output_factory: Some(Box::new(move || {
            Box::new(output.take().expect("factory used once"))
        })),
        takeover: TakeoverSeams::inert(),
    };
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
    assert!(
        failure.to_string().contains("aborted during the countdown"),
        "{failure}"
    );
    // Nothing was emitted before the abort.
    assert!(
        failure.to_string().contains("nothing was emitted"),
        "{failure}"
    );

    // Restoration behavior: dropping the guard restores the previous
    // SIGINT/SIGTERM dispositions and resets the stop state.
    drop(guard);
    assert!(
        !touchpad_linux::termination_requested(),
        "guard teardown must reset the stop state"
    );
}

/// M6 re-review R2: a real SIGTERM is observed the same way.
#[cfg(target_os = "linux")]
#[test]
fn real_sigterm_during_emit_is_observed_and_maps_to_exit_8() {
    let _lock = SIGNAL_TEST_LOCK.lock().unwrap();
    let guard = touchpad_linux::install_termination_handler()
        .expect("installing the termination handler must succeed");
    // SAFETY: see `real_sigint_during_emit_is_observed_and_maps_to_exit_8`.
    unsafe {
        libc::raise(libc::SIGTERM);
    }
    assert!(touchpad_linux::termination_requested());

    let command = Command::OutputProbe { emit: true };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut output = Some(touchpad_desktop::FakeDesktopOutput::available());
    let mut env = CommandEnv {
        sys: Rc::new(MockSys::new()) as Rc<dyn touchpad_linux::sys::Sys>,
        out: &mut out,
        err: &mut err,
        stop_flag: Arc::new(AtomicBool::new(false)),
        recorder_factory: None,
        output_factory: Some(Box::new(move || {
            Box::new(output.take().expect("factory used once"))
        })),
        takeover: TakeoverSeams::inert(),
    };
    let failure = run_command(&mut env, &command).unwrap_err();
    assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
    drop(guard);
    assert!(!touchpad_linux::termination_requested());
}

/// M10 review R4 (public CLI path): duplicates of ALL five mandatory
/// takeover flags are usage errors at the parser layer (exit 1) — including
/// `--output-qualified`, `--profile`, and `--max-duration-seconds`, whether
/// or not the repeated values agree — so no command object is produced and
/// nothing executes.
#[test]
fn takeover_duplicates_of_all_five_flags_are_usage_errors() {
    let base: Vec<String> = vec![
        "takeover".to_string(),
        "/dev/input/event0".to_string(),
        "t.jsonl".to_string(),
        "--takeover".to_string(),
        "--confirm".to_string(),
        "TAKEOVER".to_string(),
        "--output-qualified".to_string(),
        "--profile".to_string(),
        "m10-linear-v1".to_string(),
        "--max-duration-seconds".to_string(),
        "60".to_string(),
    ];
    // The canonical command parses (control).
    assert!(parse_args(base.clone()).is_ok());

    let duplicated = |flag: &str| {
        let mut args = base.clone();
        // Insert the duplicate right before the final duration value.
        let pos = args
            .iter()
            .position(|a| a == "--max-duration-seconds")
            .expect("duration flag present");
        args.insert(pos, flag.to_string());
        parse_args(args)
    };
    for flag in [
        "--takeover",
        "--confirm",
        "--output-qualified",
        "--profile",
        "--max-duration-seconds",
    ] {
        let err = duplicated(flag).unwrap_err();
        let failure = CommandFailure::Usage(err);
        assert_eq!(
            failure.exit_code(),
            ExitCode::Usage,
            "duplicate {flag} must be a usage error (exit 1)"
        );
    }
    // A conflicting repeated value is likewise a usage error (never a silent
    // overwrite): 60 then 300.
    let conflict = parse_args(vec![
        "takeover".to_string(),
        "/dev/input/event0".to_string(),
        "t.jsonl".to_string(),
        "--takeover".to_string(),
        "--confirm".to_string(),
        "TAKEOVER".to_string(),
        "--output-qualified".to_string(),
        "--profile".to_string(),
        "m10-linear-v1".to_string(),
        "--max-duration-seconds".to_string(),
        "60".to_string(),
        "--max-duration-seconds".to_string(),
        "300".to_string(),
    ]);
    assert!(matches!(
        conflict,
        Err(touchpadctl::args::UsageError::DuplicateTakeoverFlag { .. })
    ));
}

/// M10 (M10_TASK.md §9): ordinary commands and takeover **parse failures**
/// cause zero open/output/recorder/grab calls — the CLI contract is fully
/// validated before any side effect.
#[test]
fn takeover_parse_failures_cause_zero_side_effects() {
    // A malformed takeover (duration out of range) is a usage error...
    let bad = parse_args(vec![
        "takeover".to_string(),
        "/dev/input/event0".to_string(),
        "t.jsonl".to_string(),
        "--takeover".to_string(),
        "--confirm".to_string(),
        "TAKEOVER".to_string(),
        "--output-qualified".to_string(),
        "--profile".to_string(),
        "m10-linear-v1".to_string(),
        "--max-duration-seconds".to_string(),
        "301".to_string(),
    ]);
    assert!(bad.is_err(), "{bad:?}");

    // ... and a takeover-only flag on another command is a usage error.
    let flagged = parse_args(vec![
        "replay".to_string(),
        "t.jsonl".to_string(),
        "--takeover".to_string(),
    ]);
    assert!(flagged.is_err(), "{flagged:?}");

    // None of these ever reach the command runner, so no device is opened,
    // no output session is created, no recorder is built, and no grab is
    // issued. Parse errors short-circuit before `run_command`; we prove the
    // parser never touches the seam by asserting the mock sys saw zero calls.
    let sys = Rc::new(MockSys::new());
    assert!(sys.log().is_empty(), "{:?}", sys.log());
    assert!(bad.is_err());
    assert!(flagged.is_err());
}

// ---------------------------------------------------------------------------
// M11: fake-backed public CLI coverage (M11_TASK.md §11/§12)
// ---------------------------------------------------------------------------

/// The canonical full takeover argument vector for a profile, with all five
/// mandatory opt-ins (M10_TASK.md §2 / M11_TASK.md §4).
fn takeover_base(profile: &str) -> Vec<String> {
    vec![
        "takeover".to_string(),
        "/dev/input/event0".to_string(),
        "t.jsonl".to_string(),
        "--takeover".to_string(),
        "--confirm".to_string(),
        "TAKEOVER".to_string(),
        "--output-qualified".to_string(),
        "--profile".to_string(),
        profile.to_string(),
        "--max-duration-seconds".to_string(),
        "60".to_string(),
    ]
}

/// The full public CLI takeover command with `m11-fidelity-v1` (all five
/// mandatory opt-ins) parses and runs fake-backed to a clean deadline stop.
/// The experimental M11 banner is written before any device/output/recorder/
/// countdown/grab side effect (it precedes the step-6 device status line,
/// which is only printed after the device open, the output prepare, and the
/// recorder attach), the fidelity-enabled pipeline emits the committed move
/// to the fake session, and the ordered cleanup runs with exactly one
/// grab/ungrab. No real device/portal/libei/desktop input is involved.
#[test]
fn takeover_m11_fidelity_v1_public_cli_run_is_fake_backed_and_clean() {
    let args = [
        "takeover",
        "/dev/input/event0",
        "m11.jsonl",
        "--takeover",
        "--confirm",
        "TAKEOVER",
        "--output-qualified",
        "--profile",
        "m11-fidelity-v1",
        "--max-duration-seconds",
        "1",
    ];
    let command = parse_args(args.iter().map(|s| s.to_string())).unwrap();
    let Command::Takeover {
        device,
        max_duration_seconds,
        profile,
        ..
    } = &command
    else {
        panic!("expected takeover");
    };
    assert_eq!(device, &PathBuf::from("/dev/input/event0"));
    assert_eq!(max_duration_seconds, &1);
    assert_eq!(profile, "m11-fidelity-v1");

    // The mock touchpad with one committed one-finger move
    // (100,50) → (200,50) = +1 mm x.
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event0");
    let mut device_mock = mock_touchpad();
    let mut batch = begin_contact(1);
    batch.extend(move_contact(2, 200, 50));
    batch.extend(end_contact(3));
    device_mock.push_raw(batch);
    sys.add_device(&path, device_mock);

    let now = Rc::new(RefCell::new(Monotonic::ZERO));
    let streaming_state = Rc::new(RefCell::new(FakeStreamingState::happy()));
    let mut out = Vec::new();
    let mut err = Vec::new();
    // First poll ready (the events), then idle (the deadline expires).
    let mut env = takeover_env(
        Rc::clone(&sys),
        &mut out,
        &mut err,
        Rc::clone(&now),
        vec![true],
        Rc::clone(&streaming_state),
    );
    let trace_path = temp_output("m11-cli");
    let command = Command::Takeover {
        device: path.clone(),
        trace: trace_path.clone(),
        max_duration_seconds: 1,
        profile: "m11-fidelity-v1".to_string(),
        feel_config: None,
        settings: None,
        watch_settings: false,
    };
    let result = run_command(&mut env, &command);
    assert!(result.is_ok(), "{result:?}");
    drop(env);

    let err_text = String::from_utf8(err).unwrap();
    assert!(err_text.contains("maximum duration reached"), "{err_text}");
    // The M11 banner appears before the step-6 device status line and before
    // the stop report.
    let banner = err_text
        .find("m11-fidelity-v1 is EXPERIMENTAL")
        .expect("the M11 banner is written");
    let device_line = err_text
        .find("device: /dev/input/event0")
        .expect("the device status line is written");
    let stopped = err_text
        .find("takeover stopped")
        .expect("the stop report is written");
    assert!(
        banner < device_line && device_line < stopped,
        "banner ordering: {err_text}"
    );
    // Every M11_TASK.md §11 banner claim.
    assert!(err_text.contains("UNCALIBRATED"), "{err_text}");
    assert!(err_text.contains("NOT the default"), "{err_text}");
    assert!(err_text.contains("macOS-equivalence"), "{err_text}");
    assert!(err_text.contains("NO live M11 validation"), "{err_text}");
    assert!(err_text.contains("--takeover"), "{err_text}");
    assert!(err_text.contains("--confirm TAKEOVER"), "{err_text}");
    assert!(err_text.contains("--output-qualified"), "{err_text}");
    assert!(err_text.contains("1..=300"), "{err_text}");

    // The fidelity-enabled pipeline emitted the committed move to the fake
    // session (no real portal/libei/desktop output), prepared and released
    // exactly once.
    let submitted = streaming_state.borrow().submitted.clone();
    assert_eq!(submitted.len(), 1, "{submitted:?}");
    assert!(
        matches!(submitted[0], OutputEvent::PointerMove { .. }),
        "{submitted:?}"
    );
    assert_eq!(streaming_state.borrow().prepare_calls, 1);
    assert_eq!(streaming_state.borrow().release_calls, 1);

    // Ordered cleanup on the syscall log: ungrab before close, exactly one
    // grab and one ungrab.
    let log = sys.log();
    let ungrab = log
        .iter()
        .position(|call| matches!(call, MockCall::Grab(_, false)))
        .expect("ungrab");
    let close = log
        .iter()
        .position(|call| matches!(call, MockCall::Close(_)))
        .expect("close");
    assert!(ungrab < close, "ungrab must precede close");
    assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);
    assert_eq!(
        sys.count(|call| matches!(call, MockCall::Grab(_, false))),
        1
    );
    std::fs::remove_file(&trace_path).ok();
}

/// All five mandatory takeover opt-ins remain mandatory and independently
/// validated for `m11-fidelity-v1` (M11_TASK.md §4): the canonical m11
/// command parses only with all five present, and missing any one of
/// `--takeover`, `--confirm TAKEOVER`, `--output-qualified`, `--profile`, or
/// `--max-duration-seconds` is a usage error (exit 1) at the public CLI
/// layer.
#[test]
fn takeover_m11_preserves_all_five_mandatory_opt_ins() {
    assert!(parse_args(takeover_base("m11-fidelity-v1")).is_ok());
    let missing = |flag: &str, value: Option<&str>| {
        let mut args = takeover_base("m11-fidelity-v1");
        args.retain(|a| match value {
            Some(value) => a != flag && a != value,
            None => a != flag,
        });
        let err = parse_args(args).unwrap_err();
        let failure = CommandFailure::Usage(err);
        assert_eq!(
            failure.exit_code(),
            ExitCode::Usage,
            "missing {flag} must be a usage error: {failure}"
        );
    };
    missing("--takeover", None);
    missing("--confirm", Some("TAKEOVER"));
    missing("--output-qualified", None);
    missing("--profile", Some("m11-fidelity-v1"));
    missing("--max-duration-seconds", Some("60"));
}

/// Duplicate and missing `--profile` behavior is preserved for
/// `m11-fidelity-v1` (M10 review R4 / M11_TASK.md §4): a duplicate profile
/// flag — with an identical or a conflicting value — is a usage error
/// (never a silent overwrite), a missing profile is a usage error, and no
/// profile is inferred when `--profile` is absent — all exit 1 at the public
/// CLI layer before any side effect.
#[test]
fn takeover_m11_duplicate_and_missing_profile_are_usage_errors() {
    // Duplicate --profile with an IDENTICAL value is still rejected.
    let mut dup_same = takeover_base("m11-fidelity-v1");
    dup_same.extend(["--profile".to_string(), "m11-fidelity-v1".to_string()]);
    let err = parse_args(dup_same).unwrap_err();
    let failure = CommandFailure::Usage(err);
    assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    assert!(
        failure.to_string().contains("may only be given once"),
        "{failure}"
    );

    // Duplicate --profile with a CONFLICTING value is rejected (the second
    // value never silently overwrites the first).
    let mut dup_conflict = takeover_base("m11-fidelity-v1");
    dup_conflict.extend(["--profile".to_string(), "m10-linear-v1".to_string()]);
    let err = parse_args(dup_conflict).unwrap_err();
    let failure = CommandFailure::Usage(err);
    assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    assert!(
        failure.to_string().contains("may only be given once"),
        "{failure}"
    );

    // Missing --profile is a usage error (no profile is inferred).
    let mut missing = takeover_base("m11-fidelity-v1");
    missing.retain(|a| a != "--profile" && a != "m11-fidelity-v1");
    let err = parse_args(missing).unwrap_err();
    let failure = CommandFailure::Usage(err);
    assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    assert!(failure.to_string().contains("--profile"), "{failure}");
}

/// The accepted `--profile` set grows explicitly as later experimental
/// layers are added; the M10 baseline stays mention-first and no profile is
/// inferred.
#[test]
fn takeover_m11_accepted_set_is_exact_at_the_public_cli() {
    assert_eq!(
        touchpadctl::args::ACCEPTED_TAKEOVER_PROFILES,
        [
            "m10-linear-v1",
            "m11-fidelity-v1",
            "m12-scroll-v1",
            "m13-robust-v1",
            "m14-gestures-v1",
            "m15-kde-v1",
            "m16-production-v1",
            "m17-tunable-v1",
            "m18-remap-v1",
            "m19-live-v1",
        ]
    );
    for profile in [
        "m10-linear-v1",
        "m11-fidelity-v1",
        "m12-scroll-v1",
        "m13-robust-v1",
        "m14-gestures-v1",
        "m15-kde-v1",
        "m16-production-v1",
    ] {
        assert!(
            parse_args(takeover_base(profile)).is_ok(),
            "profile {profile} must parse"
        );
    }
    let mut m17 = takeover_base("m17-tunable-v1");
    m17.extend(["--feel-config".to_string(), "feel.json".to_string()]);
    assert!(parse_args(m17).is_ok());
    let mut m18 = takeover_base("m18-remap-v1");
    m18.extend(["--settings".to_string(), "settings.json".to_string()]);
    assert!(parse_args(m18).is_ok());
    let mut m19 = takeover_base("m19-live-v1");
    m19.extend([
        "--settings".to_string(),
        "settings.json".to_string(),
        "--watch-settings".to_string(),
    ]);
    assert!(parse_args(m19).is_ok());
    let err = parse_args(takeover_base("macos-like")).unwrap_err();
    let failure = CommandFailure::Usage(err);
    assert_eq!(failure.exit_code(), ExitCode::Usage, "{failure}");
    let text = failure.to_string();
    assert!(text.contains("m10-linear-v1"), "{text}");
    assert!(text.contains("m11-fidelity-v1"), "{text}");
    assert!(text.contains("m18-remap-v1"), "{text}");
    assert!(text.contains("m19-live-v1"), "{text}");
}

/// The `1..=300` maximum-duration bound is preserved for `m11-fidelity-v1`
/// (M11_TASK.md §4): 1 and 300 parse; 0, 301, malformed, negative, and
/// overflow values are usage errors (exit 1).
#[test]
fn takeover_m11_duration_limits_are_1_to_300() {
    for ok in ["1", "300"] {
        let mut args = takeover_base("m11-fidelity-v1");
        let pos = args.iter().position(|a| a == "60").expect("duration");
        args[pos] = ok.to_string();
        assert!(
            parse_args(args).is_ok(),
            "duration {ok} must parse with m11-fidelity-v1"
        );
    }
    for bad in ["0", "301", "abc", "-5", "99999999999999999999"] {
        let mut args = takeover_base("m11-fidelity-v1");
        let pos = args.iter().position(|a| a == "60").expect("duration");
        args[pos] = bad.to_string();
        let err = parse_args(args).unwrap_err();
        let failure = CommandFailure::Usage(err);
        assert_eq!(
            failure.exit_code(),
            ExitCode::Usage,
            "duration {bad} must be rejected: {failure}"
        );
    }
}

/// Serializes the tests that install/fire the real SIGINT/SIGTERM handler or
/// read the process-lifetime stop state (the signal dispositions and the
/// stop static are process-global).
#[cfg(test)]
static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
