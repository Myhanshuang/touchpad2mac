//! `touchpadctl` binary entry point (M5).
//!
//! Thin shell around the library command runner: parse `argv`, build the
//! real [`touchpad_linux::sys`] seam, install the `SIGINT`/`SIGTERM` handler
//! for `record` (recording a stop request in a process-lifetime static, M5
//! re-review R1), run the command, and exit with the structured exit code.
//!
//! No panic is expected on any path: every command maps failures to
//! [`touchpadctl::CommandFailure`] with a stable exit code.

use std::io::Write;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use touchpad_core::Monotonic;
use touchpad_linux::sys::{Fd, Sys};
use touchpadctl::env::{ClockFn, ReadinessFn, TakeoverSeams};
use touchpadctl::{parse_args, run_command, CommandEnv, ExitCode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = real_main(args);
    std::process::exit(code);
}

fn real_main(args: Vec<String>) -> i32 {
    let command = match parse_args(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{}", touchpadctl::args::HELP_TEXT);
            return ExitCode::Usage.code();
        }
    };

    let mut out = std::io::stdout();
    let mut err = std::io::stderr();

    // The SIGINT/SIGTERM handler is installed only for commands with a
    // blocking wait and ordered cleanup: `record` (device read; the handler
    // records a stop request in a process-lifetime static — M5 re-review
    // R1) and `output-probe --emit` (real desktop emission whose ordered
    // cleanup — release held state, disconnect, close session — must run on
    // a real Ctrl-C/SIGTERM, M6 re-review R2). The guard restores the
    // previous dispositions on drop. Every other command keeps the default
    // dispositions, so Ctrl+C retains its ordinary terminate behavior (the
    // dry-run `output-probe` holds nothing that needs cleanup).
    let _signal_guard: Option<touchpad_linux::TerminationHandlerGuard> =
        if touchpadctl::command_needs_termination_handler(&command) {
            match touchpad_linux::install_termination_handler() {
                Ok(guard) => Some(guard),
                Err(error) => {
                    writeln!(
                        err,
                        "touchpadctl: could not install signal handling: {error}"
                    )
                    .ok();
                    return ExitCode::Unexpected.code();
                }
            }
        } else {
            None
        };

    // Injectable stop source (the real signal handler uses the
    // process-lifetime static observed via `touchpad_linux::termination_requested`;
    // tests set this Arc to simulate a signal deterministically).
    let stop_flag = Arc::new(AtomicBool::new(false));

    let sys: Rc<dyn Sys> = real_sys();
    let takeover = real_takeover_seams(Rc::clone(&sys));
    let mut env = CommandEnv {
        sys,
        out: &mut out,
        err: &mut err,
        stop_flag,
        recorder_factory: None,
        output_factory: Some(Box::new(real_output)),
        takeover,
    };

    match run_command(&mut env, &command) {
        Ok(()) => ExitCode::Success.code(),
        Err(failure) => {
            writeln!(err, "touchpadctl: {failure}").ok();
            failure.exit_code().code()
        }
    }
}

/// The real desktop output backend for `output-probe` (M6): the
/// portal/libei backend on Linux, an honest unsupported fallback elsewhere.
fn real_output() -> Box<dyn touchpad_desktop::DesktopOutput> {
    #[cfg(target_os = "linux")]
    {
        Box::new(touchpad_desktop::PortalDesktopOutput::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(touchpad_desktop::UnsupportedDesktopOutput)
    }
}

/// The real OS seam on Linux; on non-Linux targets every live command fails
/// with an actionable "no such device" result (the offline replay path works
/// everywhere).
fn real_sys() -> Rc<dyn Sys> {
    #[cfg(target_os = "linux")]
    {
        Rc::new(touchpad_linux::sys::ffi::LinuxSys::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Rc::new(touchpad_linux::sys::mock::MockSys::new())
    }
}

/// The real M10 takeover seams (M10_TASK.md §7): a wall-free monotonic clock
/// for the deadline, a `poll(2)`-based bounded-readiness seam on the session
/// fd (through the `Sys` seam, whose Linux implementation lives inside
/// `touchpad-linux`'s existing unsafe FFI boundary), and a real sleeper for
/// the countdown. The production streaming backend is selected inside the
/// takeover command after profile/settings validation so M19 can choose the
/// KDE desktop-action composite while tests keep their injected factory.
fn real_takeover_seams(sys: Rc<dyn Sys>) -> TakeoverSeams {
    let clock: ClockFn = {
        let epoch = std::time::Instant::now();
        Rc::new(move || {
            let elapsed = epoch.elapsed().as_nanos();
            Monotonic::from_nanos(u64::try_from(elapsed).unwrap_or(u64::MAX))
        })
    };
    let readiness: ReadinessFn = Rc::new(move |fd: Fd, timeout: Duration| sys.poll(fd, timeout));
    TakeoverSeams {
        clock,
        readiness,
        sleeper: Rc::new(std::thread::sleep),
        streaming_factory: None,
    }
}
