//! `touchpadctl devices` — enumerate `/dev/input/event*` and explain each
//! verdict.

use std::io::Write;

use touchpad_linux::device::{ProbeError, ProbeReport, ProbeVerdict};
use touchpad_linux::sys::SysError;
use touchpad_linux::{enumerate, pick_candidate};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

/// Runs `devices`.
///
/// Every enumerated node is printed with its verdict and evidence; the
/// command succeeds (exit 0) when at least one candidate was found, and
/// fails with a clear, actionable message (exit 2 missing directory, 3
/// permission, 4 no candidate) otherwise. Never panics.
pub fn run(env: &mut CommandEnv<'_>) -> Result<(), CommandFailure> {
    let reports = match enumerate(&*env.sys) {
        Ok(reports) => reports,
        Err(ProbeError::ReadDir { path, source }) => return Err(classify_read_dir(path, source)),
    };

    writeln!(
        env.out,
        "input event nodes in /dev/input: {}",
        reports.len()
    )
    .map_err(output_error)?;
    for (index, report) in reports.iter().enumerate() {
        print_report(&mut *env.out, index, report).map_err(output_error)?;
    }

    match pick_candidate(&reports) {
        Some(index) => {
            writeln!(
                env.err,
                "candidate touchpad: {} ({})",
                reports[index].path.display(),
                reports[index].name
            )
            .map_err(output_error)?;
            Ok(())
        }
        None => Err(CommandFailure::NoCandidate(if reports.is_empty() {
            "no /dev/input/event* nodes were found on this system. If you \
             expected a touchpad, check that the input subsystem is present \
             and that this environment exposes input devices."
                .to_string()
        } else {
            "no touchpad candidate was found among the enumerated event \
             nodes. If you expected one, check that the device is a Type-B \
             multitouch pointer/buttonpad device and that your user can read \
             /dev/input/event* (usually the `input` group)."
                .to_string()
        })),
    }
}

/// Classifies a directory-read failure into an actionable command failure
/// with a stable exit code.
fn classify_read_dir(path: std::path::PathBuf, source: SysError) -> CommandFailure {
    match source {
        SysError::NotFound { path } => CommandFailure::InputDir(format!(
            "no input directory {}: /dev/input does not exist on this system \
             (no input subsystem or a container without device nodes)",
            path.display()
        )),
        SysError::PermissionDenied { path, .. } => CommandFailure::Permission(format!(
            "permission denied reading {}: check that your user can read \
             /dev/input (usually membership in the `input` group)",
            path.display()
        )),
        other => CommandFailure::Unexpected(format!("could not read {}: {other}", path.display())),
    }
}

/// Prints one probe report: path, name, verdict, and its evidence.
fn print_report(out: &mut dyn Write, index: usize, report: &ProbeReport) -> std::io::Result<()> {
    let verdict = match &report.verdict {
        ProbeVerdict::Candidate { .. } => "candidate",
        ProbeVerdict::Rejected { .. } => "rejected",
        ProbeVerdict::Inaccessible { .. } => "inaccessible",
    };
    writeln!(
        out,
        "[{}] {} — {:?} — {verdict}",
        index + 1,
        report.path.display(),
        report.name
    )?;
    match &report.verdict {
        ProbeVerdict::Candidate { descriptor } => {
            writeln!(
                out,
                "      candidate: Type-B multitouch pointer device, slot_count={}, physical_buttons={}",
                descriptor.slot_count.unwrap_or_default(),
                descriptor.has_physical_buttons
            )?;
        }
        ProbeVerdict::Rejected { reasons } => {
            for reason in reasons {
                writeln!(out, "      rejected: {reason}")?;
            }
        }
        ProbeVerdict::Inaccessible { error } => {
            writeln!(out, "      inaccessible: {error}")?;
        }
    }
    for evidence in &report.evidence {
        writeln!(out, "      evidence: {evidence}")?;
    }
    Ok(())
}

fn output_error(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("could not write output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::rc::Rc;

    use touchpad_linux::sys::mock::{MockDevice, MockFailure, MockSys};

    use crate::env::TakeoverSeams;

    use crate::env::CommandEnv;

    fn env<'a>(sys: Rc<MockSys>, out: &'a mut Vec<u8>, err: &'a mut Vec<u8>) -> CommandEnv<'a> {
        CommandEnv {
            sys: sys as Rc<dyn touchpad_linux::sys::Sys>,
            out,
            err,
            stop_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recorder_factory: None,
            output_factory: None,
            takeover: TakeoverSeams::inert(),
        }
    }

    #[test]
    fn no_devices_is_a_clear_result() {
        let sys = Rc::new(MockSys::new());
        sys.set_dir_entries(vec![]);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env).unwrap_err();
        assert_eq!(failure.exit_code(), crate::exit::ExitCode::NoCandidate);
        assert!(failure.to_string().contains("no /dev/input/event* nodes"));
        assert!(String::from_utf8(out).unwrap().contains("0"));
    }

    #[test]
    fn missing_input_dir_is_actionable() {
        let sys = Rc::new(MockSys::new());
        sys.set_read_dir_error(MockFailure::NotFound);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env).unwrap_err();
        assert_eq!(failure.exit_code(), crate::exit::ExitCode::InputDir);
        assert!(failure.to_string().contains("/dev/input"), "{failure}");
    }

    #[test]
    fn permission_denied_on_input_dir_is_actionable() {
        let sys = Rc::new(MockSys::new());
        sys.set_read_dir_error(MockFailure::PermissionDenied);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env).unwrap_err();
        assert_eq!(failure.exit_code(), crate::exit::ExitCode::Permission);
        assert!(failure.to_string().contains("permission"), "{failure}");
    }

    #[test]
    fn mixed_verdicts_are_printed_and_candidate_wins() {
        let sys = Rc::new(MockSys::new());
        sys.set_dir_entries(vec![
            PathBuf::from("/dev/input/event0"),
            PathBuf::from("/dev/input/event1"),
        ]);
        let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
        touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
        touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
            !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
        sys.add_device(PathBuf::from("/dev/input/event0"), touchscreen);
        sys.add_device(
            PathBuf::from("/dev/input/event1"),
            MockDevice::touchpad("Pad", 10),
        );
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        run(&mut env).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("rejected"), "{text}");
        assert!(text.contains("candidate"), "{text}");
        let err_text = String::from_utf8(err).unwrap();
        assert!(err_text.contains("/dev/input/event1"), "{err_text}");
    }
}
