//! `touchpadctl replay INPUT` — offline replay of a raw trace through the
//! exact same Type-B decoder used by live input.
//!
//! Output contract (stable and testable):
//!
//! * **stdout**: exactly one JSON object per committed [`ContactFrame`]
//!   (serde serialization of the core frame type), nothing else;
//! * **stderr**: human-readable summary and diagnostics;
//! * **exit code**: 0 on a clean replay, 5 for trace-file errors (missing
//!   file, corrupt line, schema mismatch, time regression, unresolved sync
//!   loss), 6 for decoder failures (e.g. a `SYN_DROPPED` resync that cannot
//!   be performed offline).
//!
//! `replay` never touches `/dev/input`: it opens the trace file only and
//! runs in ordinary-user, CI, or headless environments. It drives
//! [`touchpad_linux::TypeBDecoder`] through
//! [`touchpad_trace::ReplayDriver`] — the same decoder state machine live
//! input uses; there is no second decoder.

use std::path::Path;

use touchpad_linux::{ReplayDecodeError, TypeBDecoder};
use touchpad_trace::{ReplayDriver, ReplayError, TraceError};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;
use crate::output::FramePrinterSink;

/// Runs `replay INPUT`.
pub fn run(env: &mut CommandEnv<'_>, input: &Path) -> Result<(), CommandFailure> {
    let file = std::fs::File::open(input).map_err(|error| {
        CommandFailure::Trace(TraceError::Io(std::io::Error::new(
            error.kind(),
            format!("could not open trace file {}: {error}", input.display()),
        )))
    })?;

    let sink = FramePrinterSink::new(&mut *env.out);
    let mut decoder = TypeBDecoder::new(sink);

    match ReplayDriver::replay(file, &mut decoder) {
        Ok(stats) => {
            let sink = decoder.into_sink();
            if sink.write_failed() {
                return Err(CommandFailure::Unexpected(
                    "could not write frame output (stdout closed?)".to_string(),
                ));
            }
            writeln!(
                env.err,
                "replay summary: device={:?} schema_version={} events_forwarded={} frames={}",
                stats.header.device.name,
                stats.header.schema_version,
                stats.events_forwarded,
                sink.frames_written()
            )
            .map_err(output_error)?;
            Ok(())
        }
        Err(ReplayError::Trace(error)) => Err(CommandFailure::Trace(error)),
        Err(ReplayError::Sink(error)) => Err(sink_failure(error)),
    }
}

/// Maps a decoder replay failure to a [`CommandFailure`]: trace-declaration
/// problems (unsupported schema/clock, unrepresentable timestamp, an invalid
/// device header, or an unresolved sync loss at the end of the trace) are
/// trace errors (exit 5); a decoder failure during replay (e.g. an offline
/// `SYN_DROPPED` resync that cannot be performed without a kernel snapshot)
/// is a stream error (exit 6).
fn sink_failure(error: ReplayDecodeError) -> CommandFailure {
    match error {
        ReplayDecodeError::UnsupportedSchema(_)
        | ReplayDecodeError::UnsupportedClock
        | ReplayDecodeError::UnrepresentableTimestamp
        | ReplayDecodeError::UnresolvedSynchronizationLoss(_)
        | ReplayDecodeError::Decode(touchpad_linux::DecodeError::InvalidDevice(_)) => {
            CommandFailure::Replay(format!("replay could not decode the trace: {error}"))
        }
        other => CommandFailure::Stream(format!("replay decoder failure: {other}")),
    }
}

fn output_error(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("could not write output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};

    use crate::env::TakeoverSeams;
    use std::rc::Rc;

    use touchpad_core::DeviceDescriptor;
    use touchpad_linux::sys::mock::MockSys;
    use touchpad_trace::{TraceEvent, TraceHeader, TraceWriter};

    use crate::env::CommandEnv;
    use crate::exit::ExitCode;

    const FIXTURES_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/touchpad-trace/tests/fixtures"
    );

    fn fixture(name: &str) -> PathBuf {
        Path::new(FIXTURES_DIR).join(format!("{name}.jsonl"))
    }

    fn env<'a>(out: &'a mut Vec<u8>, err: &'a mut Vec<u8>) -> CommandEnv<'a> {
        CommandEnv {
            sys: Rc::new(MockSys::new()) as Rc<dyn touchpad_linux::sys::Sys>,
            out,
            err,
            stop_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recorder_factory: None,
            output_factory: None,
            takeover: TakeoverSeams::inert(),
        }
    }

    fn type_b_header() -> TraceHeader {
        let mut device = DeviceDescriptor::new("replay test", 0, 0);
        device.supports_type_b_mt = true;
        device.slot_count = Some(10);
        device.axes.insert(
            touchpad_core::AxisId::new(53),
            touchpad_core::AxisInfo::new(0, 1000, 0, 0, std::num::NonZeroU32::new(100)),
        );
        device.axes.insert(
            touchpad_core::AxisId::new(54),
            touchpad_core::AxisInfo::new(0, 1000, 0, 0, std::num::NonZeroU32::new(100)),
        );
        TraceHeader::new(device)
    }

    fn write_trace(path: &Path, events: &[TraceEvent]) {
        let mut writer =
            TraceWriter::new(std::fs::File::create(path).unwrap(), &type_b_header()).unwrap();
        for event in events {
            writer.write_event(event).unwrap();
        }
        writer.finish().unwrap();
    }

    /// M5 acceptance: the fixture replay smoke test — the `single_contact`
    /// fixture replays cleanly through the same decoder used live, printing
    /// one JSON ContactFrame per line on stdout and a summary on stderr.
    #[test]
    fn fixture_replay_smoke_single_contact() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        run(&mut env, &fixture("single_contact")).unwrap();

        // stdout: three JSON frames (Began / Active / Ended).
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        let frames: Vec<touchpad_core::ContactFrame> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames[0].sequence, 1);
        assert_eq!(frames[0].contacts[0].tracking_id, 10);
        assert_eq!(
            frames[0].contacts[0].state,
            touchpad_core::ContactState::Began
        );
        assert_eq!(
            frames[1].contacts[0].state,
            touchpad_core::ContactState::Active
        );
        assert_eq!(
            frames[2].contacts[0].state,
            touchpad_core::ContactState::Ended
        );

        // stderr: the summary.
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("replay summary:"), "{err_text}");
        assert!(err_text.contains("events_forwarded=10"), "{err_text}");
        assert!(err_text.contains("frames=3"), "{err_text}");
    }

    /// The other clean fixtures replay through the same decoder path too.
    #[test]
    fn clean_fixtures_replay_to_frames() {
        for name in ["multi_slot", "buttons", "missing_resolution"] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let mut env = env(&mut out, &mut err);
            run(&mut env, &fixture(name)).unwrap();
            let frames = String::from_utf8(out)
                .unwrap()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            assert!(frames > 0, "{name}: expected frames");
        }
    }

    /// A `SYN_DROPPED` trace cannot be resynchronized offline (no kernel
    /// snapshot source): the replay fails with a structured stream error.
    /// Frames decoded before the drop are still printed; nothing after it.
    #[test]
    fn dropped_recovery_fixture_fails_offline() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        let failure = run(&mut env, &fixture("dropped_recovery")).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Stream, "{failure}");
        assert!(
            failure.to_string().contains("resynchronization"),
            "{failure}"
        );
        // Only the pre-drop frame was printed (one JSON line).
        let out_text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out_text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 1, "only the pre-drop frame may be printed");
        let frame: touchpad_core::ContactFrame = serde_json::from_str(lines[0]).unwrap();
        assert!(!frame.discontinuity);
        assert_eq!(frame.sequence, 1);
    }

    /// A corrupted trace is a trace error (exit 5).
    #[test]
    fn corrupted_trace_is_a_trace_error() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{\"kind\":\"header\",\"schema_version\":1,\"clock\":\"monotonic\",\"device\":{\"name\":\"x\",\"vendor_id\":0,\"product_id\":0,\"axes\":{},\"slot_count\":10,\"supports_type_b_mt\":true,\"has_physical_buttons\":false,\"profile\":{\"name\":\"default\",\"axis_resolutions\":{},\"quirks\":[]}}}\n{{{{not json\n")
            .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        let failure = run(&mut env, &path).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
        assert!(failure.to_string().contains("corrupted"), "{failure}");
        std::fs::remove_file(&path).ok();
    }

    /// A missing trace file is a trace error (exit 5).
    #[test]
    fn missing_trace_file_is_a_trace_error() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        let failure = run(&mut env, Path::new("/no/such/trace.jsonl")).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
        assert!(
            failure.to_string().contains("could not open trace file"),
            "{failure}"
        );
    }

    /// A schema-too-new trace is rejected explicitly (exit 5).
    #[test]
    fn schema_too_new_trace_is_rejected() {
        let path = temp_path("schema");
        let mut header = type_b_header();
        header.schema_version = 2;
        // The writer refuses to emit a schema-2 header, so serialize the
        // line directly (the reader must reject it explicitly).
        std::fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string(&touchpad_trace::TraceLine::Header(header)).unwrap()
            ),
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        let failure = run(&mut env, &path).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
        assert!(failure.to_string().contains("schema version"), "{failure}");
        std::fs::remove_file(&path).ok();
    }

    /// A time regression in a trace is rejected (exit 5).
    #[test]
    fn time_regression_trace_is_rejected() {
        let path = temp_path("regression");
        write_trace(
            &path,
            &[
                TraceEvent::new(0, 2000, 3, 53, 1),
                TraceEvent::new(0, 1000, 3, 53, 2),
            ],
        );
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        let failure = run(&mut env, &path).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Trace, "{failure}");
        assert!(failure.to_string().contains("went backwards"), "{failure}");
        std::fs::remove_file(&path).ok();
    }

    /// A header-only trace replays cleanly with zero frames (exit 0).
    #[test]
    fn empty_trace_replays_cleanly_with_zero_frames() {
        let path = temp_path("empty");
        let mut writer =
            TraceWriter::new(std::fs::File::create(&path).unwrap(), &type_b_header()).unwrap();
        writer.finish().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(&mut out, &mut err);
        run(&mut env, &path).unwrap();
        assert!(String::from_utf8(out).unwrap().is_empty());
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("events_forwarded=0"), "{err_text}");
        assert!(err_text.contains("frames=0"), "{err_text}");
        std::fs::remove_file(&path).ok();
    }

    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "touchpadctl-replay-{}-{}-{}.jsonl",
            std::process::id(),
            unique,
            tag
        ))
    }
}
