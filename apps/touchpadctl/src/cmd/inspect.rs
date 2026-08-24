//! `touchpadctl inspect DEVICE` — probe one device node and show its
//! identity, capabilities, axes, slot count, and the candidate verdict.

use std::io::Write;
use std::path::Path;

use touchpad_linux::device::ProbeVerdict;
use touchpad_linux::probe;
use touchpad_linux::sys::SysError;

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

/// Runs `inspect DEVICE`.
///
/// First opens the node to classify accessibility with a stable exit code
/// (2 not found, 3 permission denied), then probes the full detail. A
/// non-candidate device is printed in full and fails with exit 4 plus the
/// rejection reasons; a candidate succeeds. Never panics.
pub fn run(env: &mut CommandEnv<'_>, device: &Path) -> Result<(), CommandFailure> {
    // Classify accessibility first (structured exit codes), then probe for
    // the full report (the probe reopens its own handle, like enumeration).
    match env.sys.open(device) {
        Err(SysError::NotFound { .. }) => {
            return Err(CommandFailure::InputDir(format!(
                "no such device node: {} — /dev/input may not exist on this \
                 system, or the device was unplugged",
                device.display()
            )));
        }
        Err(SysError::PermissionDenied { path, .. }) => {
            return Err(CommandFailure::Permission(format!(
                "permission denied reading {}: check that your user is in the \
                 `input` group or otherwise has read access to the device \
                 node (typically /dev/input/event*, mode 660 root:input)",
                path.display()
            )));
        }
        Err(other) => {
            return Err(CommandFailure::Unexpected(format!(
                "could not open {}: {other}",
                device.display()
            )));
        }
        Ok(fd) => {
            env.sys.close(fd).map_err(|error| {
                CommandFailure::Unexpected(format!(
                    "could not close {} after the accessibility check: {error}",
                    device.display()
                ))
            })?;
        }
    }

    let report = probe(&*env.sys, device);
    print_report(&mut *env.out, &report).map_err(output_error)?;

    match &report.verdict {
        ProbeVerdict::Candidate { .. } => Ok(()),
        ProbeVerdict::Rejected { reasons } => Err(CommandFailure::NoCandidate(format!(
            "device {} does not qualify as a touchpad candidate: {}",
            device.display(),
            reasons.join("; ")
        ))),
        ProbeVerdict::Inaccessible { error } => Err(CommandFailure::Stream(format!(
            "device {} became inaccessible while probing (it may have been \
             removed): {error}",
            device.display()
        ))),
    }
}

/// Prints the full probe report: identity, capabilities, axes, slot count,
/// verdict, and evidence.
fn print_report(out: &mut dyn Write, report: &touchpad_linux::ProbeReport) -> std::io::Result<()> {
    writeln!(out, "device: {}", report.path.display())?;
    writeln!(out, "  name: {:?}", report.name)?;
    writeln!(
        out,
        "  id: bustype=0x{:04x} vendor=0x{:04x} product=0x{:04x} version=0x{:04x}",
        report.id.bustype, report.id.vendor, report.id.product, report.id.version
    )?;
    let verdict = match &report.verdict {
        ProbeVerdict::Candidate { .. } => "candidate",
        ProbeVerdict::Rejected { .. } => "rejected",
        ProbeVerdict::Inaccessible { .. } => "inaccessible",
    };
    writeln!(out, "  verdict: {verdict}")?;
    writeln!(
        out,
        "  slot_count: {}",
        report.slot_count.unwrap_or_default()
    )?;
    if let ProbeVerdict::Candidate { descriptor } = &report.verdict {
        writeln!(
            out,
            "  supports_type_b_mt: {}",
            descriptor.supports_type_b_mt
        )?;
        writeln!(
            out,
            "  has_physical_buttons: {}",
            descriptor.has_physical_buttons
        )?;
    }
    if report.axes.is_empty() {
        writeln!(out, "  axes: (none reported)")?;
    } else {
        writeln!(out, "  axes:")?;
        for (axis, info) in &report.axes {
            writeln!(
                out,
                "    axis {axis:>3}: min={} max={} fuzz={} flat={} resolution={}",
                info.min, info.max, info.fuzz, info.flat, info.resolution
            )?;
        }
    }
    for reason in rejection_reasons(&report.verdict) {
        writeln!(out, "  rejected: {reason}")?;
    }
    if let ProbeVerdict::Inaccessible { error } = &report.verdict {
        writeln!(out, "  inaccessible: {error}")?;
    }
    for evidence in &report.evidence {
        writeln!(out, "  evidence: {evidence}")?;
    }
    Ok(())
}

fn rejection_reasons(verdict: &ProbeVerdict) -> &[String] {
    match verdict {
        ProbeVerdict::Rejected { reasons } => reasons,
        _ => &[],
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

    use touchpad_linux::sys::mock::{MockDevice, MockFailure, MockSys};

    use crate::env::TakeoverSeams;

    use crate::env::CommandEnv;
    use crate::exit::ExitCode;

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
    fn candidate_device_is_printed_and_succeeds() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::touchpad("Pad", 10));
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        run(&mut env, &path).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("\"Pad\""), "{text}");
        assert!(text.contains("verdict: candidate"), "{text}");
        assert!(text.contains("slot_count: 10"), "{text}");
        assert!(text.contains("axis  47"), "{text}");
    }

    #[test]
    fn non_candidate_device_fails_with_reasons() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
        touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
        touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
            !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
        sys.add_device(&path, touchscreen);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env, &path).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::NoCandidate);
        assert!(
            failure.to_string().contains("INPUT_PROP_DIRECT"),
            "{failure}"
        );
        assert!(String::from_utf8(out).unwrap().contains("rejected"));
    }

    #[test]
    fn missing_device_is_actionable() {
        let sys = Rc::new(MockSys::new());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env, Path::new("/dev/input/event9")).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::InputDir);
        assert!(
            failure.to_string().contains("no such device node"),
            "{failure}"
        );
    }

    #[test]
    fn permission_denied_device_is_actionable() {
        let sys = Rc::new(MockSys::new());
        let path = PathBuf::from("/dev/input/event0");
        sys.set_open_error(&path, MockFailure::PermissionDenied);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut env = env(sys, &mut out, &mut err);
        let failure = run(&mut env, &path).unwrap_err();
        assert_eq!(failure.exit_code(), ExitCode::Permission);
        assert!(failure.to_string().contains("permission"), "{failure}");
        assert!(failure.to_string().contains("input"), "{failure}");
    }
}
