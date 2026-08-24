//! `touchpadctl record DEVICE OUTPUT [--grab]` — record raw evdev events
//! into a versioned JSON Lines trace, with the recorder **in front of the
//! decoder** and a controlled `SIGINT`/`SIGTERM` stop.
//!
//! Pipeline (IMPLEMENTATION_BRIEF §8):
//!
//! ```text
//! read(2) batch → decode bytes → [recorder: write TraceEvent to trace]
//!              → decoder.feed → ContactFrame → counting sink (no backend)
//! ```
//!
//! The recorder write happens **before** the decoder feed for every event, so
//! a decoder bug can never lose the raw input needed to reproduce it; a
//! recorder failure is fatal for the session but never erases events already
//! recorded. Replay of the produced trace drives the **same** decoder.
//!
//! ## Preparation order (M5 review R2)
//!
//! The device is opened and validated **without grabbing**. The recorder is
//! then created from the runtime's own validated descriptor and its header is
//! flushed — a successful flush proves the output is writable — and only then
//! is the explicit optional `--grab` issued through the runtime's checked
//! [`touchpad_linux::EvdevRuntime::grab`], immediately before the read loop.
//! An unwritable output or a failed header flush therefore issues **zero**
//! `EVIOCGRAB(1)` calls.
//!
//! ## Ordered finalization (M5 re-review R3)
//!
//! Every exit path (signal, fatal stream/decoder/recorder error, failed
//! grab) runs **one** ordered finalization, performed entirely by the
//! runtime so the fallible recorder `finish` can never be postponed past the
//! device release:
//!
//! 1. stop accepting new work (the runtime phase leaves `Running`);
//! 2. semantic-output lifecycle end — an explicit no-op (Phase 1 has no real
//!    output backend);
//! 3. recorder finalization — `finish` (which flushes) **plus best-effort
//!    recorder destruction** (its `Drop` flushes buffered bytes when
//!    `finish` fails) — **before** the device release, never after
//!    ungrab/close;
//! 4. idempotent ungrab (`EVIOCGRAB(0)` at most once);
//! 5. close the fd even if the ungrab failed.
//!
//! The runtime runs this sequence on the signal and grab-failure paths via
//! [`touchpad_linux::EvdevRuntime::shutdown`] and on the fatal stream path
//! inside [`touchpad_linux::EvdevRuntime::fail_open`] (M4's immediate
//! fail-open is preserved); this command never calls the fallible `finish`
//! itself. The actual recorder-finalization, ungrab, and close results —
//! from exactly one [`touchpad_linux::ShutdownReport`], the fail-open report
//! on the fatal path or the shutdown report otherwise — feed both the
//! cleanup status line and the returned [`CommandFailure`]: exit 8
//! ([`crate::exit::CommandFailure::Stopped`]) is returned **only** when the
//! recorder finalization and the device release both succeeded; a recorder
//! finalization failure returns exit 7
//! ([`crate::exit::CommandFailure::RecorderFinalize`]) and a device-release
//! failure (ungrab/close) returns exit 6
//! ([`crate::exit::CommandFailure::DeviceRelease`]) — each message keeps the
//! primary stop reason and every cleanup diagnostic, so cleanup failures are
//! never converted into a false "trace flushed and device released" claim
//! and the status text never misreports a release as "n/a (already closed)"
//! when the actual fail-open results are known.
//!
//! `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
//! cleanup (the kernel releases the grab when the fd closes at process exit,
//! but no ordered sequence is guaranteed).

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use touchpad_linux::sys::SysError;
use touchpad_linux::{
    EvdevRuntime, GrabError, OpenError, RawEventRecorder, RecorderError, RuntimeError,
    ShutdownReport, TraceRecorder,
};
use touchpad_trace::TraceHeader;

use crate::env::CommandEnv;
use crate::exit::CommandFailure;
use crate::output::CountingSink;

/// How a record session ended.
enum StopReason {
    /// A stop was requested (SIGINT/SIGTERM) — graceful.
    Signal,
    /// A fatal stream/decoder/recorder error.
    Stream(RuntimeError),
    /// The explicit grab failed after the recorder was prepared.
    GrabFailed(RuntimeError),
}

/// Runs `record DEVICE OUTPUT [--grab]`.
pub fn run(
    env: &mut CommandEnv<'_>,
    device: &Path,
    output: &Path,
    grab: bool,
) -> Result<(), CommandFailure> {
    if grab {
        writeln!(
            env.err,
            "WARNING: --grab requested: the touchpad will be EXCLUSIVELY \
             owned by this process while recording; the system will not \
             receive its events until recording stops or the process exits."
        )
        .map_err(output_error)?;
    }

    // Open the device (validates capabilities/axes/slot on the session fd,
    // selects CLOCK_MONOTONIC, prepares the decoder and snapshot adapter).
    // `open` never grabs (M5 review R2): the grab is a separate checked step
    // below, after the recorder is prepared.
    let mut runtime = EvdevRuntime::open(Rc::clone(&env.sys), device, CountingSink::new())
        .map_err(open_failure)?;
    runtime.set_stop_flag(std::sync::Arc::clone(&env.stop_flag));

    // Recorder preparation happens BEFORE any grab (M5 review R2): build the
    // trace header from the runtime's own validated descriptor (the same
    // device model the decoder uses — no second model), create the recorder,
    // and flush the header so a failure proves the output is unwritable with
    // zero grab calls.
    let descriptor = runtime.descriptor().cloned().ok_or_else(|| {
        CommandFailure::Unexpected("runtime did not expose its device descriptor".to_string())
    })?;
    let mut recorder = create_recorder(env, output, &TraceHeader::new(descriptor))
        .map_err(CommandFailure::Recorder)?;
    if let Err(error) = recorder.flush() {
        // The header did not reach the file: the output is not writable.
        // Fail with a recorder error (exit 7); no grab was ever issued, and
        // the runtime (device open, never grabbed) drops and closes the fd
        // via its ordered fallback Drop.
        return Err(CommandFailure::Recorder(error));
    }
    runtime.set_recorder(recorder);

    // The explicit optional grab — the last preparation step, immediately
    // before the read loop. A grab failure runs the ordered finalization
    // (the device is still open and the recorder attached).
    if grab {
        if let Err(error) = runtime.grab() {
            return finalize(env, runtime, output, StopReason::GrabFailed(error));
        }
    }

    writeln!(
        env.err,
        "recording {} -> {} (grab: {grab}); stop with Ctrl-C (SIGINT) or SIGTERM",
        device.display(),
        output.display()
    )
    .map_err(output_error)?;

    // Read loop: poll the stop flag and the process-lifetime termination
    // static between steps (covers signals that arrive while the runtime is
    // not blocked in a read; the real SIGINT/SIGTERM handler records into the
    // static, M5 re-review R1); an interrupted read with a stop requested is
    // surfaced by the runtime as `RuntimeError::Interrupted` (graceful stop,
    // device left open). A failing status write above returns early and the
    // runtime's ordered fallback `Drop` performs the cleanup (M5 review R4).
    let stop_reason = loop {
        if env.stop_flag.load(Ordering::Relaxed) || touchpad_linux::termination_requested() {
            break StopReason::Signal;
        }
        match runtime.step() {
            Ok(_) => {}
            Err(RuntimeError::Interrupted) => break StopReason::Signal,
            Err(error) => break StopReason::Stream(error),
        }
    };
    finalize(env, runtime, output, stop_reason)
}

/// Builds the raw-event recorder for the session: the env's injected factory
/// (fault-injection / timeline tests) or the real
/// [`TraceRecorder::create`] (which writes the header into its buffered
/// writer — the caller flushes it to prove the output is writable).
fn create_recorder(
    env: &CommandEnv<'_>,
    output: &Path,
    header: &TraceHeader,
) -> Result<Box<dyn RawEventRecorder>, RecorderError> {
    match &env.recorder_factory {
        Some(factory) => factory(output, header),
        None => Ok(Box::new(TraceRecorder::create(output, header)?)),
    }
}

/// Runs the unified ordered finalization and returns the truthful composite
/// [`CommandFailure`] (M5 re-review R3).
///
/// The runtime performs the **whole** ordered sequence in one place: recorder
/// `finish` (which flushes) plus best-effort recorder destruction → ungrab
/// at most once → close regardless of prior errors. On the signal and
/// grab-failure paths that sequence runs here, inside
/// [`EvdevRuntime::shutdown`]; on the fatal stream path the runtime already
/// ran it inside [`EvdevRuntime::fail_open`] before `step` returned, and
/// `shutdown` is then an idempotent no-op. The command **never calls the
/// fallible `finish` itself**, so `finish` can never run after the device
/// release. The one report that carries the actual results (the fail-open
/// report on the fatal path, the shutdown report otherwise) feeds both the
/// cleanup status line and the exit decision.
fn finalize(
    env: &mut CommandEnv<'_>,
    mut runtime: EvdevRuntime<CountingSink>,
    output: &Path,
    stop_reason: StopReason,
) -> Result<(), CommandFailure> {
    // Unified ordered finalization. On the fatal path the runtime is already
    // `Stopped` (fail-open ran); on the signal/grab paths this runs the
    // sequence now.
    let report = runtime.shutdown();
    let fail_open_report = runtime.take_fail_open_report();
    let sink = runtime.into_sink();

    // The one report carrying the actual finalization results: the fail-open
    // report on the fatal path (shutdown is then a no-op), the shutdown
    // report otherwise.
    let actual = fail_open_report.unwrap_or(report);
    let events_recorded = actual.events_recorded;

    // Structured status: every finalization step's actual result is
    // reported from the same merged source as the exit decision.
    writeln!(
        env.err,
        "recording stopped: {}",
        stop_reason_text(&stop_reason)
    )
    .map_err(output_error)?;
    writeln!(
        env.err,
        "trace: {} — {events_recorded} raw events recorded, {} frames decoded ({} contacts, {} discontinuity, {} diagnostics)",
        output.display(),
        sink.frames(),
        sink.contacts(),
        sink.discontinuities(),
        sink.diagnostics()
    )
    .map_err(output_error)?;
    writeln!(
        env.err,
        "cleanup: recorder {recorder_status}, ungrab {ungrab_status}, close {close_status}",
        recorder_status = actual
            .recorder_finish
            .as_ref()
            .map(ok_err)
            .unwrap_or_else(|| "n/a (no recorder)".to_string()),
        ungrab_status = actual
            .ungrab
            .as_ref()
            .map(ok_err)
            .unwrap_or_else(|| "n/a (no grab held)".to_string()),
        close_status = actual
            .close
            .as_ref()
            .map(ok_err)
            .unwrap_or_else(|| "n/a (already closed)".to_string()),
    )
    .map_err(output_error)?;

    Err(final_failure(stop_reason, actual))
}

fn ok_err<T, E: std::fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("error ({error})"),
    }
}

/// The human-readable primary stop reason (also used by the status line).
fn stop_reason_text(stop_reason: &StopReason) -> String {
    match stop_reason {
        StopReason::Signal => "SIGINT/SIGTERM (controlled stop)".to_string(),
        StopReason::Stream(error) => error.to_string(),
        StopReason::GrabFailed(error) => error.to_string(),
    }
}

/// Decides the final [`CommandFailure`] from the primary stop reason and the
/// **one** report that carries the actual ordered-finalization results (M5
/// re-review R3).
///
/// `actual` is the fail-open report on the fatal stream path and the
/// shutdown report on the signal/grab-failure paths — exactly one source,
/// never both, never neither while a device was held. Its recorder
/// finalization, ungrab, and close results feed both the exit decision here
/// and the cleanup status line in [`finalize`], so the printed status and
/// the exit code can never disagree about what happened.
///
/// Precedence (documented): a recorder finalization failure is the most
/// actionable — the trace is not guaranteed flushed — so it wins with exit 7
/// ([`CommandFailure::RecorderFinalize`]); then a device-release failure
/// (failed ungrab and/or close) wins with exit 6
/// ([`CommandFailure::DeviceRelease`]); otherwise the primary reason decides
/// ([`CommandFailure::Stopped`], exit 8, only for a fully successful signal
/// stop). Every diagnostic — primary reason first, cleanup failures after —
/// is preserved in the returned message.
fn final_failure(stop_reason: StopReason, actual: ShutdownReport) -> CommandFailure {
    let recorder_error = actual
        .recorder_finish
        .as_ref()
        .and_then(|result| result.as_ref().err());
    let ungrab_error = actual
        .ungrab
        .as_ref()
        .and_then(|result| result.as_ref().err());
    let close_error = actual
        .close
        .as_ref()
        .and_then(|result| result.as_ref().err());
    let primary_text = stop_reason_text(&stop_reason);

    if let Some(error) = recorder_error {
        return CommandFailure::RecorderFinalize(format!(
            "trace finalization failed after {primary_text}: recorder {error}{}",
            release_suffix(ungrab_error, close_error)
        ));
    }
    if let (Some(ungrab), Some(close)) = (&actual.ungrab, &actual.close) {
        if ungrab.is_err() || close.is_err() {
            return CommandFailure::DeviceRelease(format!(
                "device release failed after {primary_text}: ungrab {}, close {}",
                ok_err(ungrab),
                ok_err(close),
            ));
        }
    }
    match stop_reason {
        StopReason::Signal => CommandFailure::Stopped,
        StopReason::Stream(error) => stream_failure(error),
        StopReason::GrabFailed(error) => stream_failure(error),
    }
}

/// Appends any device-release failures to a recorder-finalization message so
/// no diagnostic is lost (M5 review R3).
fn release_suffix(ungrab: Option<&GrabError>, close: Option<&SysError>) -> String {
    let mut parts = Vec::new();
    if let Some(error) = ungrab {
        parts.push(format!("ungrab failed: {error}"));
    }
    if let Some(error) = close {
        parts.push(format!("close failed: {error}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("; device release also failed: {}", parts.join("; "))
    }
}

/// Maps an [`EvdevRuntime::open`] failure to an actionable [`CommandFailure`]
/// with a stable exit code (2 no such node, 3 permission, 4 not a candidate,
/// 6 other device/stream errors).
fn open_failure(error: RuntimeError) -> CommandFailure {
    match error {
        RuntimeError::Open(OpenError::Access { path, source }) => match source {
            touchpad_linux::sys::SysError::NotFound { .. } => CommandFailure::InputDir(format!(
                "no such device node: {} — /dev/input may not exist on this \
                 system, or the device was unplugged",
                path.display()
            )),
            touchpad_linux::sys::SysError::PermissionDenied { path, .. } => {
                CommandFailure::Permission(format!(
                    "permission denied opening {}: check that your user is in \
                     the `input` group or otherwise has read access to the \
                     device node (typically /dev/input/event*, mode 660 \
                     root:input)",
                    path.display()
                ))
            }
            other => CommandFailure::Stream(format!("could not open {}: {other}", path.display())),
        },
        RuntimeError::Open(OpenError::NotCandidate { path, reasons }) => {
            CommandFailure::NoCandidate(format!(
                "device {} does not qualify as a touchpad candidate: {}",
                path.display(),
                reasons.join("; ")
            ))
        }
        RuntimeError::Open(OpenError::Probe { path, message }) => {
            CommandFailure::Stream(format!("could not probe {}: {message}", path.display()))
        }
        RuntimeError::Open(OpenError::Configure { path, source }) => {
            CommandFailure::Stream(format!(
                "could not configure the decoder for {}: {source}",
                path.display()
            ))
        }
        RuntimeError::Open(OpenError::SnapshotSource { message }) => CommandFailure::Stream(
            format!("could not prepare the resync snapshot source: {message}"),
        ),
        RuntimeError::Open(OpenError::Clock { path, source }) => match source {
            touchpad_linux::sys::SysError::PermissionDenied { .. } => {
                CommandFailure::Permission(format!(
                    "permission denied selecting the monotonic clock on {}: \
                         check device node permissions (the `input` group)",
                    path.display()
                ))
            }
            other => CommandFailure::Stream(format!(
                "could not select CLOCK_MONOTONIC on {}: {other}",
                path.display()
            )),
        },
        // Defensive: the runtime's open never grabs (M5 review R2); the grab
        // failure is handled by the record command after preparation.
        RuntimeError::Grab(error) => {
            CommandFailure::Stream(format!("could not grab the device (EVIOCGRAB): {error}"))
        }
        other => CommandFailure::Stream(format!("could not open the device: {other}")),
    }
}

/// Maps a fatal stream error to a [`CommandFailure`] (exit 6), except
/// recorder errors (exit 7).
fn stream_failure(error: RuntimeError) -> CommandFailure {
    match error {
        RuntimeError::Recorder(error) => CommandFailure::Recorder(error),
        other => CommandFailure::Stream(other.to_string()),
    }
}

fn output_error(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("could not write output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;

    use touchpad_core::ContactState;
    use touchpad_linux::sys::mock::{MockCall, MockDevice, MockFailure, MockSys};

    use crate::env::TakeoverSeams;
    use touchpad_linux::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, EV_ABS, EV_SYN,
        SYN_DROPPED, SYN_REPORT,
    };
    use touchpad_trace::TraceReader;

    use crate::env::CommandEnv;
    use crate::exit::ExitCode;

    fn env<'a>(sys: Rc<MockSys>, out: &'a mut Vec<u8>, err: &'a mut Vec<u8>) -> CommandEnv<'a> {
        CommandEnv {
            sys: sys as Rc<dyn touchpad_linux::sys::Sys>,
            out,
            err,
            stop_flag: std::sync::Arc::new(AtomicBool::new(false)),
            recorder_factory: None,
            output_factory: None,
            takeover: TakeoverSeams::inert(),
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

    fn temp_output(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "touchpadctl-record-{}-{}-{}.jsonl",
            std::process::id(),
            unique,
            tag
        ))
    }

    /// Reads the trace file back with the reader and returns its events.
    fn read_trace_events(path: &Path) -> Vec<touchpad_trace::TraceEvent> {
        let mut reader = TraceReader::new(std::fs::File::open(path).unwrap());
        reader.read_header().unwrap();
        reader.events().map(Result::unwrap).collect()
    }

    /// M5 acceptance: a normal record session writes the raw events to the
    /// trace (recorder before decoder), decodes frames, and cleans up.
    #[test]
    fn record_writes_events_decodes_frames_and_cleans_up() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_raw(begin_contact(1));
        sys.add_device(&path, device);
        let output = temp_output("ok");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        // First step records and decodes; the (empty) read stream then hits
        // EOF -> device stream error, after the ordered cleanup.
        let failure = run(&mut env, &path, &output, false).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");

        // The trace holds the 5 raw events.
        let events = read_trace_events(&output);
        assert_eq!(events.len(), 5);
        // The status reported the recorded events and frames. The fatal
        // (EOF) path merged the fail-open report into the cleanup line, so
        // the actual ungrab/close results are printed (M5 re-review R3) —
        // never "n/a (already closed)".
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("5 raw events recorded"), "{err_text}");
        assert!(err_text.contains("1 frames decoded"), "{err_text}");
        assert!(
            err_text.contains("cleanup: recorder ok, ungrab ok, close ok"),
            "{err_text}"
        );
        // No grab was requested (--grab defaults off).
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 0);
        std::fs::remove_file(&output).ok();
    }

    /// M5 acceptance: --grab is an explicit opt-in that grabs the device.
    #[test]
    fn grab_flag_grabs_the_device_explicitly() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_raw(begin_contact(1));
        sys.add_device(&path, device);
        let output = temp_output("grab");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        // The device was grabbed once and released exactly once (fail-open).
        assert_eq!(sys.count(|call| matches!(call, MockCall::Grab(_, true))), 1);
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("WARNING: --grab"), "{err_text}");
        assert!(err_text.contains("EXCLUSIVELY"), "{err_text}");
        std::fs::remove_file(&output).ok();
    }

    /// M5 acceptance: a decoder failure must not lose the raw events already
    /// read — the trace holds every event of the batch, including the ones
    /// around the failing feed.
    #[test]
    fn decoder_failure_does_not_lose_recorded_raw_events() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        let mut batch = begin_contact(1);
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0));
        device.push_raw(batch);
        // The resync snapshot query fails -> the decoder degrades on the
        // recovery SYN_REPORT (fatal decoder failure).
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let output = temp_output("decode-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, &output, false).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");

        // All 7 events of the batch were recorded despite the decoder
        // failure (recorder runs before the decoder feed).
        let events = read_trace_events(&output);
        assert_eq!(
            events.len(),
            7,
            "no read event may be lost to a decoder bug"
        );
        // The grab was released (fail-open cleanup ran).
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("cleanup:"), "{err_text}");
        std::fs::remove_file(&output).ok();
    }

    /// M5 acceptance: a stop flag set before the first step stops the
    /// session gracefully (exit 8), with the ordered cleanup (recorder flush
    /// -> ungrab -> close) and a flushed trace.
    #[test]
    fn stop_flag_stops_gracefully_with_ordered_cleanup() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_raw(begin_contact(1));
        sys.add_device(&path, device);
        let output = temp_output("signal");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");

        // The trace was flushed (header-only: no read happened).
        let events = read_trace_events(&output);
        assert!(events.is_empty());
        let err_text = String::from_utf8(err).unwrap();
        assert!(
            err_text.contains("SIGINT/SIGTERM (controlled stop)"),
            "{err_text}"
        );
        assert!(err_text.contains("0 raw events recorded"), "{err_text}");
        assert!(
            err_text.contains("cleanup: recorder ok, ungrab ok, close ok"),
            "{err_text}"
        );

        // Ordered cleanup on the syscall log: ungrab then close, exactly
        // once each (the grab was held).
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

    /// M5 acceptance: an EINTR with the stop flag set is a graceful stop
    /// (not an ordinary fatal error) — exit 8 and the ordered shutdown.
    #[test]
    fn eintr_with_stop_flag_is_a_graceful_stop() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_read_failure(MockFailure::Interrupted);
        sys.add_device(&path, device);
        let output = temp_output("eintr");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stopped, "{failure}");
        let err_text = String::from_utf8(err).unwrap();
        assert!(
            err_text.contains("SIGINT/SIGTERM (controlled stop)"),
            "{err_text}"
        );
        assert!(
            err_text.contains("cleanup: recorder ok, ungrab ok, close ok"),
            "{err_text}"
        );
        std::fs::remove_file(&output).ok();
    }

    /// M5: an EINTR WITHOUT a stop request keeps the M4 semantics — an
    /// ordinary fatal error (exit 6), fail-open cleanup.
    #[test]
    fn eintr_without_stop_flag_is_a_fatal_stream_error() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_read_failure(MockFailure::Interrupted);
        sys.add_device(&path, device);
        let output = temp_output("eintr-fatal");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("EINTR"), "{err_text}");
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5: opening a non-candidate device fails with the rejection reasons
    /// (exit 4).
    #[test]
    fn record_rejects_non_candidate_device_with_reasons() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
        touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
        touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
            !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
        sys.add_device(&path, touchscreen);
        let output = temp_output("rejected");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, &output, false).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::NoCandidate, "{failure}");
        assert!(
            failure.to_string().contains("INPUT_PROP_DIRECT"),
            "{failure}"
        );
        // No trace file was created for a rejected device.
        assert!(!output.exists());
    }

    /// M5: opening a missing device node is actionable (exit 2).
    #[test]
    fn record_missing_device_is_actionable() {
        let sys = Rc::new(MockSys::new());
        let output = temp_output("missing");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, Path::new("/dev/input/event9"), &output, false).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::InputDir, "{failure}");
        assert!(
            failure.to_string().contains("no such device node"),
            "{failure}"
        );
    }

    /// M5 review R2: an unwritable recorder output is a recorder error
    /// (exit 7) with **zero** grab calls — the grab is only issued after the
    /// output was created and its header flushed.
    #[test]
    fn recorder_output_failure_is_actionable_with_zero_grabs() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad());
        let output = Path::new("/definitely/not/a/real/directory/xyz/trace.jsonl");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
        // The output could not be created: no grab was ever issued, and the
        // runtime's fallback Drop closed the (never grabbed) device fd.
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, true))),
            0,
            "an unwritable output must issue zero grab calls"
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
    }

    /// M5: the produced trace is replayable through the same decoder used by
    /// live input — a recorded contact decodes to a ContactFrame.
    #[test]
    fn recorded_trace_replays_through_the_same_decoder() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_raw(begin_contact(1));
        sys.add_device(&path, device);
        let output = temp_output("replayable");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let _ = run(&mut env, &path, &output, false);

        // Replay the trace offline through the TypeBDecoder (same path as
        // live input) and check the frame.
        let mut decoder =
            touchpad_linux::TypeBDecoder::new(touchpad_linux::RecordingFrameSink::new());
        touchpad_trace::ReplayDriver::replay(std::fs::File::open(&output).unwrap(), &mut decoder)
            .unwrap();
        let frames = decoder.into_sink().frames().to_vec();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].contacts.len(), 1);
        assert_eq!(frames[0].contacts[0].tracking_id, 7);
        assert_eq!(frames[0].contacts[0].state, ContactState::Began);
        std::fs::remove_file(&output).ok();
    }

    // ---------------------------------------------------------------------
    // M5 review R2/R3/R4: preparation order, composite failures, fallback
    // ---------------------------------------------------------------------

    use std::cell::RefCell;

    use touchpad_linux::sys::{AbsInfo, Fd, InputId, SysError};
    use touchpad_linux::{KernelEvent, RawEventRecorder, RecorderError};

    /// A recorder that records markers into a shared timeline, so command
    /// activity can be interleaved with the syscall log.
    struct MarkerRecorder {
        timeline: Rc<RefCell<Vec<String>>>,
        events: u64,
    }

    impl RawEventRecorder for MarkerRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
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

    /// A recorder whose header flush always fails (M5 review R2/R3).
    struct FlushFailingRecorder;

    impl RawEventRecorder for FlushFailingRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
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

    /// A recorder whose finish always fails (M5 review R3).
    struct FinishFailingRecorder;

    impl RawEventRecorder for FinishFailingRecorder {
        fn record(&mut self, _event: &KernelEvent) -> Result<(), RecorderError> {
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

    /// A `Write` that fails after `remaining` successful writes (fault
    /// injection for the record command's status output, M5 review R4).
    struct FailAfterWrites {
        remaining: usize,
    }

    impl std::io::Write for FailAfterWrites {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::other("injected output failure"));
            }
            self.remaining -= 1;
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A sys seam that records a marker for every call into a shared
    /// timeline, delegating behavior to a `MockSys` (command-level analogue
    /// of the runtime's own `TimelineSys`).
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

        fn ioctl_id(&self, fd: Fd) -> Result<InputId, SysError> {
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

        fn ioctl_absinfo(&self, fd: Fd, abs_code: u16) -> Result<AbsInfo, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("absinfo({fd:?}, {abs_code})"));
            self.inner.ioctl_absinfo(fd, abs_code)
        }

        fn ioctl_mt_slots(&self, fd: Fd, buf: &mut [i32]) -> Result<(), SysError> {
            self.timeline.borrow_mut().push(format!("mtslots({fd:?})"));
            self.inner.ioctl_mt_slots(fd, buf)
        }

        fn poll(&self, fd: Fd, timeout: std::time::Duration) -> Result<bool, SysError> {
            self.timeline
                .borrow_mut()
                .push(format!("poll({fd:?}, {:?})", timeout));
            self.inner.poll(fd, timeout)
        }
    }

    /// M5 review R2: in one shared timeline the recorder/header preparation
    /// (create + flush) precedes the explicit `EVIOCGRAB(1)`, which precedes
    /// the first read — grab is the last preparation step.
    #[test]
    fn header_flush_precedes_grab_in_the_shared_timeline() {
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn touchpad_linux::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.push_raw(begin_contact(1));
        mock.add_device(&path, device);
        let output = temp_output("order");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let timeline_for_factory = Rc::clone(&timeline);
        let mut env = CommandEnv {
            sys,
            out: &mut out,
            err: &mut err,
            stop_flag: std::sync::Arc::new(AtomicBool::new(false)),
            recorder_factory: Some(Box::new(move |_, _| {
                Ok(Box::new(MarkerRecorder {
                    timeline: Rc::clone(&timeline_for_factory),
                    events: 0,
                }))
            })),
            output_factory: None,
            takeover: TakeoverSeams::inert(),
        };
        // The stream ends in EOF after the contact -> a stream exit; the
        // timeline order below is what this test proves.
        let _ = run(&mut env, &path, &output, true);

        let timeline = timeline.borrow();
        let flush = timeline
            .iter()
            .position(|marker| marker == "recorder:flush")
            .expect("header flush in timeline");
        let grab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", true)"))
            .expect("grab in timeline");
        let read = timeline
            .iter()
            .position(|marker| marker.starts_with("read("))
            .expect("read in timeline");
        assert!(
            flush < grab,
            "the header flush must precede EVIOCGRAB(1): {timeline:?}"
        );
        assert!(
            grab < read,
            "EVIOCGRAB(1) must precede the first read: {timeline:?}"
        );
        std::fs::remove_file(&output).ok();
    }

    /// M5 review R2: a header flush failure (the output is not writable)
    /// aborts with a recorder error (exit 7) and **zero** grab calls.
    #[test]
    fn header_flush_failure_issues_zero_grabs() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad());
        let output = temp_output("flush-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.recorder_factory = Some(Box::new(|_, _| Ok(Box::new(FlushFailingRecorder))));
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
        assert!(
            failure.to_string().contains("injected flush failure"),
            "{failure}"
        );
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, true))),
            0,
            "a header flush failure must issue zero grab calls"
        );
        // The runtime's fallback Drop still closed the (never grabbed) fd.
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5 review R3: a recorder finish failure while stopping returns the
    /// accurate recorder exit (7) — never a false exit 8 — and still
    /// performs the ordered device release (ungrab then close, exactly once
    /// each).
    #[test]
    fn recorder_finish_failure_returns_recorder_exit_with_cleanup() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, mock_touchpad());
        let output = temp_output("finish-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        env.recorder_factory = Some(Box::new(|_, _| Ok(Box::new(FinishFailingRecorder))));
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
        let message = failure.to_string();
        assert!(message.contains("trace finalization failed"), "{message}");
        assert!(message.contains("SIGINT/SIGTERM"), "{message}");
        assert!(message.contains("injected finish failure"), "{message}");
        // The device was still released in order, each operation exactly
        // once.
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

    /// M5 review R3: a failed ungrab with a successful close returns the
    /// accurate device-release failure (exit 6) with both diagnostics,
    /// instead of a false exit 8; the release is attempted at most once and
    /// the close still runs.
    #[test]
    fn failed_ungrab_with_successful_close_is_reported() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let output = temp_output("ungrab-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        let message = failure.to_string();
        assert!(message.contains("device release failed"), "{message}");
        assert!(message.contains("SIGINT/SIGTERM"), "{message}");
        assert!(message.contains("ungrab error"), "{message}");
        assert!(message.contains("close ok"), "{message}");
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1,
            "a failed release is attempted at most once"
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5 review R3: a close failure is reported (exit 6) with the ungrab
    /// diagnostic preserved; the close is still attempted exactly once.
    #[test]
    fn close_failure_is_reported() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        device.close_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let output = temp_output("close-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.stop_flag.store(true, Ordering::Relaxed);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        let message = failure.to_string();
        assert!(message.contains("device release failed"), "{message}");
        assert!(message.contains("ungrab ok"), "{message}");
        assert!(message.contains("close error"), "{message}");
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5 review R3: a primary decoder/stream failure combined with a
    /// cleanup failure preserves BOTH diagnostics — the message carries the
    /// decoder error and the failed ungrab — and the release order/count
    /// stays correct.
    #[test]
    fn primary_stream_error_combined_with_cleanup_failure_preserves_both() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        let mut batch = begin_contact(1);
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0));
        device.push_raw(batch);
        device.mt_slots_error = Some(MockFailure::Io);
        device.release_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let output = temp_output("combined");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        let message = failure.to_string();
        assert!(
            message.contains("resynchronization"),
            "the primary decoder error must be preserved: {message}"
        );
        assert!(message.contains("device release failed"), "{message}");
        assert!(message.contains("ungrab error"), "{message}");
        assert_eq!(
            sys.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(sys.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5 review R4: a failing status writer after the recorder was attached
    /// (an early `?` return) still runs the ordered fallback cleanup —
    /// recorder finalization (finish + destruction) before ungrab before
    /// close in one shared timeline, each device operation at most once —
    /// and the command returns the output failure.
    #[test]
    fn status_writer_failure_after_recorder_attachment_uses_ordered_fallback() {
        let mock = Rc::new(MockSys::new());
        let timeline = Rc::new(RefCell::new(Vec::new()));
        let sys = Rc::new(TimelineSys {
            inner: Rc::clone(&mock),
            timeline: Rc::clone(&timeline),
        }) as Rc<dyn touchpad_linux::sys::Sys>;
        let path = PathBuf::from("/dev/input/event0");
        mock.add_device(&path, mock_touchpad());
        let output = temp_output("status-fail");

        let mut out = Vec::new();
        // grab=true writes the WARNING line first (succeeds), then the
        // "recording ..." status line written after the recorder was
        // attached fails — the early `?` return that exercises the fallback.
        let mut err = FailAfterWrites { remaining: 1 };
        let timeline_for_factory = Rc::clone(&timeline);
        let mut env = CommandEnv {
            sys,
            out: &mut out,
            err: &mut err,
            stop_flag: std::sync::Arc::new(AtomicBool::new(false)),
            recorder_factory: Some(Box::new(move |_, _| {
                Ok(Box::new(MarkerRecorder {
                    timeline: Rc::clone(&timeline_for_factory),
                    events: 0,
                }))
            })),
            output_factory: None,
            takeover: TakeoverSeams::inert(),
        };
        let failure = run(&mut env, &path, &output, true).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Unexpected, "{failure}");

        // The fallback Drop completed the recorder's fallible finalization
        // (finish; the last finalization marker in the timeline) before the
        // device release; ungrab then close, each exactly once.
        let timeline = timeline.borrow();
        let last_finish = timeline
            .iter()
            .rposition(|marker| marker == "recorder:finish")
            .expect("recorder finish in timeline");
        let ungrab = timeline
            .iter()
            .position(|marker| marker.starts_with("grab(") && marker.ends_with(", false)"))
            .expect("ungrab in timeline");
        let close = timeline
            .iter()
            .position(|marker| marker.starts_with("close("))
            .expect("close in timeline");
        assert!(
            last_finish < ungrab,
            "fallback Drop must finish the recorder before the ungrab: {timeline:?}"
        );
        assert!(
            ungrab < close,
            "fallback Drop must ungrab before close: {timeline:?}"
        );
        assert_eq!(
            mock.count(|call| matches!(call, MockCall::Grab(_, true))),
            1
        );
        assert_eq!(
            mock.count(|call| matches!(call, MockCall::Grab(_, false))),
            1
        );
        assert_eq!(mock.count(|call| matches!(call, MockCall::Close(_))), 1);
        std::fs::remove_file(&output).ok();
    }

    /// M5 re-review R3: a **fatal primary failure combined with a recorder
    /// finalization failure** preserves both diagnostics and the exit
    /// precedence — the recorder finalization failure wins (exit 7), the
    /// primary decoder error stays in the message, the cleanup line reports
    /// the actual fatal-path results, and the device is still released in
    /// order (ungrab then close, exactly once each).
    #[test]
    fn fatal_primary_failure_combined_with_recorder_finalization_failure() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut device = mock_touchpad();
        let mut batch = begin_contact(1);
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0));
        batch.extend(ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0));
        device.push_raw(batch);
        // The resync snapshot query fails -> fatal decoder error.
        device.mt_slots_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let output = temp_output("fatal-finish-fail");

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys.clone(), &mut out, &mut err);
        env.recorder_factory = Some(Box::new(|_, _| Ok(Box::new(FinishFailingRecorder))));
        let failure = run(&mut env, &path, &output, true).unwrap_err();

        // Precedence: the recorder finalization failure (exit 7) wins over
        // the fatal primary stream error.
        assert_eq!(failure.exit_code(), ExitCode::Recorder, "{failure}");
        let message = failure.to_string();
        assert!(
            message.contains("trace finalization failed"),
            "the recorder finalization failure must be reported: {message}"
        );
        assert!(
            message.contains("resynchronization"),
            "the primary decoder error must be preserved: {message}"
        );
        assert!(
            message.contains("injected finish failure"),
            "the finish diagnostic must be preserved: {message}"
        );

        // The cleanup line reports the actual fail-open results (the fatal
        // path's recorder finalization failure and the successful release).
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("cleanup: recorder error"), "{err_text}");
        assert!(
            err_text.contains("ungrab ok, close ok"),
            "the actual fatal-path release results must be printed: {err_text}"
        );

        // The device was still released in order, each operation exactly
        // once (fail-open ran the full sequence despite the finish failure).
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
}
