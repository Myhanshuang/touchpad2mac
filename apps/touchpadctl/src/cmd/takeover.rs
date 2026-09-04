//! `touchpadctl takeover` — the M10 bounded, fail-open live takeover slice
//! (M10_TASK.md).
//!
//! ```text
//! explicit evdev device (exclusive grab)
//!   → existing Type-B decoder/resync
//!   → approved M7–M9 Arbiter/ArbiterSink (m10-linear-v1)
//!   → prepared portal + libei streaming OutputSink
//!   → current KDE Wayland desktop
//! ```
//!
//! # Preparation order — grab is the final irreversible step (M10_TASK.md §4)
//!
//! ```text
//! 1. parse/validate the complete CLI contract            (args.rs, zero side effects)
//! 2. open + validate the evdev device, select the clock  (no read, no grab)
//! 3. prepare a reusable streaming portal/libei session   (cancellable, bounded;
//!                                                         requires relative pointer,
//!                                                         primary/secondary button,
//!                                                         pixel-precise scroll)
//! 4. construct the M7–M9 arbiter pipeline (m10-linear-v1) (the decoder's sink)
//! 5. create the mandatory trace recorder from the exact validated descriptor,
//!    flush its header, attach it before any raw event can reach the decoder
//! 6. print the resolved device/trace/profile/capabilities/duration/cleanup
//!    order/escape instructions; run a visible cancellable countdown (≥ 3 s)
//! 7. re-check stop/readiness; then exactly one EVIOCGRAB(1), immediately
//!    before the bounded event loop
//! ```
//!
//! Any failure/cancel before step 7 issues **zero grabs and zero semantic
//! desktop events**; a prepared output session is still explicitly released
//! and the opened device/recorder still finalized/closed in the ordered
//! coordinator path with diagnostics preserved.
//!
//! # Truly bounded event loop (M10_TASK.md §7)
//!
//! The maximum duration expires even when the touchpad produces no input: the
//! loop wakes at a short fixed quantum ([`POLL_QUANTUM`]) through the
//! injectable readiness seam, checks the injected monotonic clock (deadline),
//! the signal stop, the bridge fault, and then reads only when ready. Tests
//! use a fake clock/readiness and never sleep. The grab may exceed the
//! configured limit by at most the documented polling quantum.
//!
//! # Readiness classification (M10 review R1/R2)
//!
//! The readiness seam classifies `poll(2)` revents explicitly:
//!
//! * `POLLIN`/`POLLHUP`/`POLLERR` mean a read on the fd would make progress
//!   (data, or the real EOF/error of an unplugged/failed device), so the
//!   loop reads immediately instead of idling until the deadline;
//! * `POLLNVAL` is an immediate structured poll failure (the fd is invalid);
//! * a pure timeout is idle (the loop re-checks the clock/stop/fault).
//!
//! An `EINTR` from `poll(2)` is re-checked against the stop sources: with
//! the installed non-`SA_RESTART` handler a normal Ctrl-C/SIGTERM while the
//! loop is idle interrupts the poll, so a **requested** stop is the
//! documented controlled signal stop (clean, exit 0), while an **unrequested**
//! EINTR keeps its M4/M5 semantics as an actionable poll/stream failure
//! (exit 6).
//!
//! [`EvdevRuntime::step_deferred`] gives the loop a deferred-cleanup step
//! path: fatal stream/decoder/recorder errors stop new work but leave the
//! output, recorder, grab, and fd available to this coordinator's **unified
//! ordered shutdown** — the existing immediate fail-open would release the
//! device before the virtual output cleanup, which M10 must not do.
//!
//! # One unified ordered shutdown (M10_TASK.md §8)
//!
//! Every post-preparation exit (deadline, SIGINT/SIGTERM, output/arbiter
//! fault, portal revocation, EOF/unplug, poll/read error, decoder degraded /
//! resync failure, recorder failure, grab failure, status-writer failure, or
//! panic fallback) converges on the idempotent [`finalize`]:
//!
//! ```text
//! 1. stop accepting raw/semantic work
//! 2. ArbiterSink::release_all — release owed virtual Left/Right and scroll
//!    lifecycle, then the wrapped portal sink disconnects and closes its session
//! 3. finalize/destroy the recorder (finish result preserved)
//! 4. EVIOCGRAB(0) at most once
//! 5. close the device fd exactly once, even if the ungrab failed
//! ```
//!
//! For pre-grab failures the same order applies to the resources that exist,
//! with zero ungrab ioctl if no grab was acquired. A repeated shutdown is a
//! full no-op. The structured outcome carries the primary stop reason and
//! **all** cleanup failures (every explicit virtual release, the wrapped
//! output cleanup, the recorder finish, the ungrab, the close, and the
//! status-output failures). Exit-code precedence is deterministic and
//! documented (see [`finalize`] and the help text); a controlled deadline or
//! signal is reported as clean (exit 0) only when every required cleanup
//! succeeded.
//!
//! `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
//! cleanup: the kernel releases the grab when the fd closes at process exit,
//! but no ordered sequence is guaranteed. No live claim is made by this
//! milestone — M10 stays live-unqualified until the user completes the
//! 10/60/300-second acceptance sequence (`doc/old/acceptance/M10_ACCEPTANCE.md`).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use touchpad_core::{
    ArbiterConfig, ArbiterSinkError, FeelConfig, M10Profile, M11Profile, M12Profile, M13Profile,
    M14Profile, M15Profile, M16Profile, M17Profile, M18Profile, M19Profile, Monotonic,
    UserSettings,
};
use touchpad_desktop::capabilities::Capability;
use touchpad_desktop::{DesktopOutputError, StreamingOutput};
use touchpad_linux::sys::{Fd, SysError};
use touchpad_linux::{
    discover_keyboards, enumerate, EvdevRuntime, KeyboardMonitor, OpenError, ProbeError,
    ProbeVerdict, RawEventRecorder, RecorderError, RuntimeError, ShutdownReport, TakeoverBridge,
};
use touchpad_trace::TraceHeader;

use crate::desktop_backend::RealDesktopPlan;
use crate::env::CommandEnv;
use crate::exit::CommandFailure;

/// Prints a status line to stderr; a write failure stops the session with a
/// [`StopReason::StatusOutput`] reason that still runs the ordered
/// [`finalize`] (the guard must already exist at the use site).
macro_rules! status {
    ($env:expr, $guard:expr, $($arg:tt)*) => {{
        if let Err(error) = writeln!($env.err, $($arg)*) {
            return finalize($env, &mut $guard, StopReason::StatusOutput(error));
        }
    }};
}

/// Maps the takeover loop's process-relative scheduling clock onto the
/// kernel/trace monotonic time domain used by accepted input frames.
///
/// The deadline clock may have any monotonic origin. Live evdev frames use
/// kernel `CLOCK_MONOTONIC` since boot, while the real CLI deadline clock is
/// `Instant::elapsed()` since process start. Runtime-generated momentum ticks
/// therefore anchor both domains after each successful input step and add
/// only the elapsed process duration to the latest accepted input timestamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InputDomainTickClock {
    process_anchor: Option<Monotonic>,
    input_anchor: Option<Monotonic>,
    input_sequence: Option<u64>,
}

impl InputDomainTickClock {
    fn observe_input(
        &mut self,
        process_now: Monotonic,
        input_sequence: Option<u64>,
        input_now: Option<Monotonic>,
    ) {
        let (Some(input_sequence), Some(input_now)) = (input_sequence, input_now) else {
            return;
        };
        if self.input_sequence == Some(input_sequence) {
            return;
        }
        self.process_anchor = Some(process_now);
        self.input_anchor = Some(input_now);
        self.input_sequence = Some(input_sequence);
    }

    fn map_process_now(&self, process_now: Monotonic) -> Option<Monotonic> {
        let process_anchor = self.process_anchor?;
        let input_anchor = self.input_anchor?;
        let elapsed = process_now.duration_since(process_anchor)?;
        Some(input_anchor.saturating_add(elapsed))
    }
}

/// A selected takeover profile (M11_TASK.md §11): the canonical name, the
/// validated arbiter configuration the pipeline runs with, the banner written
/// **before** any device/output/recorder/countdown/grab side effect, and the
/// short profile description used by the step-6 status line. Selection and
/// banner construction are pure — no device, output session, recorder,
/// countdown, or grab is touched — so [`select_profile`] is directly testable
/// without entering `run`'s side effects.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedProfile {
    /// The canonical versioned profile name.
    pub name: &'static str,
    /// The validated arbiter configuration (M10: exactly `M10Profile`'s
    /// config; M11: exactly `M11Profile`'s config, i.e. the M10 config plus
    /// the fidelity stage).
    pub arbiter_config: ArbiterConfig,
    /// The banner written before any side effect.
    pub banner: String,
    /// The short profile description used by the step-6 status line.
    pub description: String,
}

/// Failure of [`select_profile`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProfileSelectionError {
    /// The name is not in the accepted set.
    #[error(
        "unknown policy profile {found:?}; accepted profiles are `m10-linear-v1`, `m11-fidelity-v1`, `m12-scroll-v1`, `m13-robust-v1`, `m14-gestures-v1`, `m15-kde-v1`, `m16-production-v1`, `m17-tunable-v1`, `m18-remap-v1`, and `m19-live-v1`"
    )]
    Unknown {
        /// The profile name that was given.
        found: String,
    },
    /// A documented profile constant failed its own validation (a
    /// programming error; the constants are chosen to validate).
    #[error("policy profile {name} failed its own validation: {message}")]
    Invalid {
        /// The canonical profile name.
        name: &'static str,
        /// The underlying construction error.
        message: String,
    },
    /// M17 selection requires a validated explicit tuning overlay.
    #[error("m17-tunable-v1 requires an explicit validated FeelConfig")]
    MissingFeelConfig,
    /// Earlier profiles cannot be mutated by a feel overlay.
    #[error("FeelConfig is only accepted by m17-tunable-v1")]
    UnexpectedFeelConfig,
    /// M18/M19 selection requires a validated explicit settings document.
    #[error("m18-remap-v1/m19-live-v1 requires an explicit validated UserSettings")]
    MissingSettings,
    /// Earlier profiles cannot be mutated by M18/M19 settings.
    #[error("UserSettings is only accepted by m18-remap-v1 or m19-live-v1")]
    UnexpectedSettings,
}

/// Selects the takeover profile by name, constructing **exactly** the
/// matching profile's arbiter configuration and the banner to print before
/// any side effect (M11_TASK.md §11). Pure and testable without entering
/// `takeover::run`.
pub fn select_profile(profile_name: &str) -> Result<SelectedProfile, ProfileSelectionError> {
    select_profile_with_overlays(profile_name, None, None)
}

/// Pure profile selector that accepts the already-loaded M17 tuning overlay.
pub fn select_profile_with_feel(
    profile_name: &str,
    feel: Option<FeelConfig>,
) -> Result<SelectedProfile, ProfileSelectionError> {
    select_profile_with_overlays(profile_name, feel, None)
}

/// Pure M18 profile selector with an already-loaded settings document.
pub fn select_profile_with_settings(
    profile_name: &str,
    settings: Option<UserSettings>,
) -> Result<SelectedProfile, ProfileSelectionError> {
    select_profile_with_overlays(profile_name, None, settings)
}

fn select_profile_with_overlays(
    profile_name: &str,
    feel: Option<FeelConfig>,
    settings: Option<UserSettings>,
) -> Result<SelectedProfile, ProfileSelectionError> {
    if profile_name != touchpad_core::m17::M17_TUNABLE_V1_NAME && feel.is_some() {
        return Err(ProfileSelectionError::UnexpectedFeelConfig);
    }
    if !matches!(
        profile_name,
        touchpad_core::m18::M18_REMAP_V1_NAME | touchpad_core::m19::M19_LIVE_V1_NAME
    ) && settings.is_some()
    {
        return Err(ProfileSelectionError::UnexpectedSettings);
    }
    match profile_name {
        touchpad_core::m10::M10_LINEAR_V1_NAME => {
            let profile = M10Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M10Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M10Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: format!(
                    "profile: {} ({})",
                    M10Profile::NAME,
                    M10_PROFILE_DESCRIPTION
                ),
                description: M10_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m11::M11_FIDELITY_V1_NAME => {
            let profile = M11Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M11Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M11Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M11_EXPERIMENTAL_BANNER.to_string(),
                description: M11_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m12::M12_SCROLL_V1_NAME => {
            let profile = M12Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M12Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M12Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M12_EXPERIMENTAL_BANNER.to_string(),
                description: M12_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m13::M13_ROBUST_V1_NAME => {
            let profile = M13Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M13Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M13Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M13_EXPERIMENTAL_BANNER.to_string(),
                description: M13_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m14::M14_GESTURES_V1_NAME => {
            let profile = M14Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M14Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M14Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M14_EXPERIMENTAL_BANNER.to_string(),
                description: M14_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m15::M15_KDE_V1_NAME => {
            let profile = M15Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M15Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M15Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M15_EXPERIMENTAL_BANNER.to_string(),
                description: M15_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m16::M16_PRODUCTION_V1_NAME => {
            let profile = M16Profile::new().map_err(|error| ProfileSelectionError::Invalid {
                name: M16Profile::NAME,
                message: error.to_string(),
            })?;
            Ok(SelectedProfile {
                name: M16Profile::NAME,
                arbiter_config: profile.arbiter_config(),
                banner: M16_EXPERIMENTAL_BANNER.to_string(),
                description: M16_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m17::M17_TUNABLE_V1_NAME => {
            let feel = feel.ok_or(ProfileSelectionError::MissingFeelConfig)?;
            let profile =
                M17Profile::with_feel(feel).map_err(|error| ProfileSelectionError::Invalid {
                    name: M17Profile::NAME,
                    message: error.to_string(),
                })?;
            Ok(SelectedProfile {
                name: M17Profile::NAME,
                arbiter_config: profile.arbiter_config().map_err(|error| {
                    ProfileSelectionError::Invalid {
                        name: M17Profile::NAME,
                        message: error.to_string(),
                    }
                })?,
                banner: M17_EXPERIMENTAL_BANNER.to_string(),
                description: M17_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m18::M18_REMAP_V1_NAME => {
            let settings = settings.ok_or(ProfileSelectionError::MissingSettings)?;
            let profile =
                M18Profile::new(settings).map_err(|error| ProfileSelectionError::Invalid {
                    name: M18Profile::NAME,
                    message: error.to_string(),
                })?;
            Ok(SelectedProfile {
                name: M18Profile::NAME,
                arbiter_config: profile.arbiter_config().map_err(|error| {
                    ProfileSelectionError::Invalid {
                        name: M18Profile::NAME,
                        message: error.to_string(),
                    }
                })?,
                banner: M18_EXPERIMENTAL_BANNER.to_string(),
                description: M18_PROFILE_DESCRIPTION.to_string(),
            })
        }
        touchpad_core::m19::M19_LIVE_V1_NAME => {
            let settings = settings.ok_or(ProfileSelectionError::MissingSettings)?;
            let profile =
                M19Profile::new(settings).map_err(|error| ProfileSelectionError::Invalid {
                    name: M19Profile::NAME,
                    message: error.to_string(),
                })?;
            Ok(SelectedProfile {
                name: M19Profile::NAME,
                arbiter_config: profile.arbiter_config().map_err(|error| {
                    ProfileSelectionError::Invalid {
                        name: M19Profile::NAME,
                        message: error.to_string(),
                    }
                })?,
                banner: M19_EXPERIMENTAL_BANNER.to_string(),
                description: M19_PROFILE_DESCRIPTION.to_string(),
            })
        }
        other => Err(ProfileSelectionError::Unknown {
            found: other.to_string(),
        }),
    }
}

/// The M10 baseline profile description (mention-first in help/errors).
const M10_PROFILE_DESCRIPTION: &str = "linear one-finger pointer, tap/tap-and-drag/drag-lock, two-finger 2D natural scroll, secondary tap, buttonpad two-finger click";

/// The M11 profile description for the step-6 status line.
const M11_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL one-finger pointer fidelity (signed radial dead zone, time-domain velocity, smoothstep gain, tracking multiplier) layered on the M10 interaction policy";

/// The M11 experimental banner, written before any device/output/recorder/
/// countdown/grab side effect. It explicitly states (M11_TASK.md §11):
///
/// * experimental and uncalibrated;
/// * not the default;
/// * no macOS equivalence claim;
/// * no live M11 validation has occurred;
/// * all M10 safety opt-ins and the 1..=300 second bound still apply.
const M11_EXPERIMENTAL_BANNER: &str = "WARNING: profile m11-fidelity-v1 is EXPERIMENTAL and UNCALIBRATED. It is NOT the default profile and makes NO macOS-equivalence claim. NO live M11 validation has occurred: M11 stays live-unqualified until a separate, later M11-specific user acceptance is written and passed. All M10 safety opt-ins (--takeover, --confirm TAKEOVER, --output-qualified) and the 1..=300 second maximum-duration bound still apply.";

const M12_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL two-finger scroll fidelity (time-domain velocity, smoothstep gain, axis lock, software momentum) layered on M11/M10";

const M12_EXPERIMENTAL_BANNER: &str = "WARNING: profile m12-scroll-v1 is EXPERIMENTAL and UNCALIBRATED. It is NOT the default profile and makes NO macOS-equivalence claim. Software scroll momentum is enabled but has NO live M12 validation yet. M10/M11 safety and qualification boundaries remain in force, including all takeover opt-ins and the 1..=300 second duration bound.";

const M13_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL feature-aware palm/thumb/edge/typing/jitter robustness layered on M12";

const M13_EXPERIMENTAL_BANNER: &str = "WARNING: profile m13-robust-v1 is EXPERIMENTAL and feature-dependent. Missing contact size, pressure, orientation, edge geometry, or typing signals use explicit fallbacks rather than fabricated data. It is NOT the default and makes NO macOS-equivalence claim. NO live M13 validation has occurred; all bounded takeover safety rules remain in force.";

const M14_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL continuous pinch/rotate/page/edge/three/four-finger gesture semantics layered on M13";

const M14_EXPERIMENTAL_BANNER: &str = "WARNING: profile m14-gestures-v1 is EXPERIMENTAL. Continuous gesture semantics are platform-neutral; the M6 pointer backend explicitly reports them unavailable unless a dedicated desktop adapter is configured. It is NOT the default, makes NO macOS-equivalence claim, and has NO live M14 validation. All bounded takeover safety rules remain in force.";

const M15_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL three-finger drag/drag-lock and platform-neutral desktop actions with configurable KDE mapping";

const M15_EXPERIMENTAL_BANNER: &str = "WARNING: profile m15-kde-v1 is EXPERIMENTAL. Three-finger drag is implemented in core, while KDE desktop actions require a separately configured action transport; no real KDE action transport is enabled by default. It is NOT the default, makes NO macOS-equivalence claim, and has NO live M15 validation. All bounded takeover safety rules remain in force.";

const M16_PROFILE_DESCRIPTION: &str =
    "M16 configuration-complete Phase-2 policy inheriting M15 interactions; productionization contracts remain foreground-only and live-unqualified";

const M16_EXPERIMENTAL_BANNER: &str = "WARNING: profile m16-production-v1 means the M12-M16 configuration stack is code-complete, NOT live-production-qualified. It remains foreground-only, live-unqualified, and makes NO macOS-equivalence or cross-device claim. All M10 bounded takeover opt-ins and duration limits remain mandatory.";

const M17_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL explicitly tuned pointer/scroll/gesture/three-finger-drag feel overlay on M16";

const M17_EXPERIMENTAL_BANNER: &str = "WARNING: profile m17-tunable-v1 uses an explicit user-edited FeelConfig. Tuning changes pointer/scroll/gesture/drag feel only; it does NOT weaken M10 takeover safety, cleanup or qualification rules. M17 is EXPERIMENTAL, never inferred, makes NO macOS-equivalence claim, and remains live-unqualified.";

const M18_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL M17 feel tuning plus configurable gesture-to-desktop-action routing";

const M18_EXPERIMENTAL_BANNER: &str = "WARNING: profile m18-remap-v1 is EXPERIMENTAL and user-configured. Gesture mappings resolve only to typed built-in DesktopAction semantics or passthrough/disabled; arbitrary shell commands are not supported. A real desktop-action transport may still be unavailable. M10 bounded takeover safety remains mandatory and M18 is live-unqualified.";

const M19_PROFILE_DESCRIPTION: &str =
    "EXPERIMENTAL M18 user settings with neutral-boundary live hot reload";

const M19_EXPERIMENTAL_BANNER: &str = "WARNING: profile m19-live-v1 is EXPERIMENTAL and LIVE-RELOADS the explicit settings file. On the real KDE Plasma backend, supported DesktopAction events are emitted through KGlobalAccel while pointer/button/scroll stay on portal+libei; unsupported mappings are rejected before grab or rejected on reload while keeping last-good. Valid saves are applied only at a neutral interaction boundary. It does not weaken M10 cleanup/safety, starts no daemon/network listener, and remains live-unqualified.";

/// The type alias for the takeover's frame sink chain: the M10 bridge over
/// the M7–M9 arbiter, whose output sink is the prepared streaming session.
type Bridge = TakeoverBridge<Box<dyn StreamingOutput>>;

/// The fixed polling quantum of the bounded takeover loop (M10_TASK.md §7):
/// the loop wakes at most every `POLL_QUANTUM` to re-check the injected
/// clock (deadline), the signal stop, and the bridge fault, even when the
/// device produces no input. The grab may exceed the configured limit only
/// by this quantum.
pub const POLL_QUANTUM: Duration = Duration::from_millis(100);

/// While M12 software scroll momentum is active, wake at most every 16 ms so
/// decay is driven smoothly without changing the M10 idle-loop bound.
pub const MOMENTUM_POLL_QUANTUM: Duration = Duration::from_millis(16);

/// The visible pre-takeover countdown length in seconds (at least 3,
/// M10_TASK.md §4 step 6).
pub const COUNTDOWN_SECONDS: u64 = 3;

/// Why the takeover session ended.
enum StopReason {
    /// SIGINT/SIGTERM observed during the bounded loop.
    Signal,
    /// The maximum duration expired during the bounded loop.
    Deadline,
    /// The user aborted before the grab (countdown cancel / signal during
    /// the countdown): nothing was grabbed and no desktop input was emitted.
    CancelledBeforeGrab,
    /// A fatal stream/decoder/recorder event error (deferred cleanup).
    Stream(RuntimeError),
    /// The explicit grab failed after all preparation succeeded.
    GrabFailed(RuntimeError),
    /// The M10 bridge stored an arbiter/output fault (M10_TASK.md §6).
    OutputFault(ArbiterSinkError),
    /// Output preparation failed (or a required capability is missing).
    Output(DesktopOutputError),
    /// The recorder could not be created or its header could not be flushed.
    RecorderPreflight(RecorderError),
    /// A status-output write failed.
    StatusOutput(std::io::Error),
}

/// Ordered best-effort fallback cleanup for the takeover coordinator
/// (M10_TASK.md §8: "panic fallback must converge on an idempotent
/// coordinator").
///
/// If the coordinator is abandoned by an early return or an unwind after
/// resources were acquired, this guard's `Drop` performs the same ordered
/// release as the explicit [`finalize`] — virtual output session release
/// first, then the recorder finalization, then the device release (ungrab →
/// close) — instead of relying on the runtime's field-drop order (which
/// would release the virtual output session only *after* the device was
/// released). The explicit [`finalize`] empties the guard first, so a
/// disarmed guard's `Drop` is a no-op (no double release).
struct TakeoverCleanup {
    runtime: Option<EvdevRuntime<Bridge>>,
    /// Read-only keyboard listeners used by DWT. They are never grabbed and
    /// are closed before the touchpad runtime is released.
    keyboards: Vec<KeyboardMonitor>,
    /// A recorder created but not yet attached to the runtime (its header
    /// flush failed): finalized by `finalize`/`Drop` before the device
    /// release.
    unattached_recorder: Option<Box<dyn RawEventRecorder>>,
}

impl Drop for TakeoverCleanup {
    fn drop(&mut self) {
        // Best-effort, ordered: release the virtual output session before the
        // device. Take the bridge (the decoder's sink) out of the runtime so
        // its release runs here, before the runtime's recorder finalization
        // and device release (which the runtime's own `Drop` performs after
        // this guard's `Drop` returns).
        let bridge = self.runtime.as_mut().and_then(EvdevRuntime::take_sink);
        if let Some(mut bridge) = bridge {
            let _ = bridge.release_all();
        }
        if let Some(mut recorder) = self.unattached_recorder.take() {
            let _ = recorder.finish();
        }
        self.keyboards.clear();
        // `self.runtime` drops here: its `Drop` finalizes the attached
        // recorder and releases the device (ungrab then close, each at most
        // once) — after the output session release above.
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProfileInputs<'a> {
    pub(crate) feel_config_path: Option<&'a Path>,
    pub(crate) settings_path: Option<&'a Path>,
    pub(crate) watch_settings: bool,
}

fn discover_unique_touchpad(env: &mut CommandEnv<'_>) -> Result<PathBuf, CommandFailure> {
    let reports = enumerate(&*env.sys).map_err(|error| match error {
        ProbeError::ReadDir { path, source } => match source {
            SysError::NotFound { path } => CommandFailure::InputDir(format!(
                "cannot auto-discover a touchpad because {} does not exist",
                path.display()
            )),
            SysError::PermissionDenied { path, .. } => CommandFailure::Permission(format!(
                "cannot auto-discover a touchpad: permission denied reading {}; check access to /dev/input (usually the input group)",
                path.display()
            )),
            other => CommandFailure::Unexpected(format!(
                "could not enumerate {} while auto-discovering the touchpad: {other}",
                path.display()
            )),
        },
    })?;

    let candidates: Vec<_> = reports
        .iter()
        .filter(|report| matches!(report.verdict, ProbeVerdict::Candidate { .. }))
        .collect();

    match candidates.as_slice() {
        [candidate] => {
            writeln!(
                env.err,
                "auto-selected touchpad: {} ({})",
                candidate.path.display(),
                candidate.name
            )
            .map_err(|error| {
                CommandFailure::Unexpected(format!("could not write device-selection status: {error}"))
            })?;
            Ok(candidate.path.clone())
        }
        [] => Err(CommandFailure::NoCandidate(
            "automatic touchpad discovery found no usable Type-B touchpad candidate. Run `touchpadctl devices` for detailed probe reasons, or use `--device /dev/input/eventX` only if you intentionally want to inspect an explicit node."
                .to_string(),
        )),
        many => {
            let mut message = String::from(
                "automatic touchpad discovery found multiple candidates; refusing to guess. Rerun takeover with exactly one `--device /dev/input/eventX` argument:\n",
            );
            for candidate in many {
                use std::fmt::Write as _;
                let _ = writeln!(
                    message,
                    "  --device {}    ({})",
                    candidate.path.display(),
                    candidate.name
                );
            }
            Err(CommandFailure::NoCandidate(message))
        }
    }
}

/// Runs `takeover TRACE [--device DEVICE] --takeover --confirm TAKEOVER
/// --output-qualified --profile m10-linear-v1 --max-duration-seconds N`.
pub(crate) fn run<'a>(
    env: &mut CommandEnv<'_>,
    device: impl Into<Option<&'a Path>>,
    trace: &Path,
    max_duration_seconds: u32,
    profile_name: &str,
    inputs: ProfileInputs<'_>,
) -> Result<(), CommandFailure> {
    run_inner(
        env,
        device.into(),
        Some(trace),
        Some(max_duration_seconds),
        profile_name,
        inputs,
        true,
    )
}

/// Runs the packaged persistent Linux service. This path deliberately reuses
/// the same device/output/arbiter/cleanup implementation as `takeover`, but
/// removes the developer-only five-minute deadline, countdown and mandatory
/// raw trace.
pub(crate) fn run_service(env: &mut CommandEnv<'_>, settings: &Path) -> Result<(), CommandFailure> {
    run_inner(
        env,
        None,
        None,
        None,
        M19Profile::NAME,
        ProfileInputs {
            feel_config_path: None,
            settings_path: Some(settings),
            watch_settings: true,
        },
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    env: &mut CommandEnv<'_>,
    device: Option<&Path>,
    trace: Option<&Path>,
    max_duration_seconds: Option<u32>,
    profile_name: &str,
    inputs: ProfileInputs<'_>,
    interactive: bool,
) -> Result<(), CommandFailure> {
    // Mandatory warnings (the CLI contract documents them; the command
    // repeats them visibly). No resources are held yet, so a status-write
    // failure here is a plain internal error.
    if let Err(error) = writeln!(
        env.err,
        "{}: the physical touchpad will be grabbed \
         EXCLUSIVELY (EVIOCGRAB), the runtime will EMIT REAL DESKTOP INPUT \
         (pointer motion, clicks, scroll) through a portal authorization \
         prompt. Keep an EXTERNAL KEYBOARD AND MOUSE available while \
         qualifying new hardware.",
        if interactive {
            "WARNING: takeover requested"
        } else {
            "service runtime starting"
        }
    ) {
        return Err(CommandFailure::Unexpected(format!(
            "could not write status: {error}"
        )));
    }
    if interactive {
        if let Err(error) = writeln!(
            env.err,
            "WARNING: --output-qualified is an OPERATOR ATTESTATION that the M6 \
             output calibration (doc/old/acceptance/M6_ACCEPTANCE.md §3) was performed. It is \
             NOT measurement evidence; the backend stays experimental/unqualified \
             and M10 stays live-unqualified until the user records the \
             10/60/300-second acceptance results (doc/old/acceptance/M10_ACCEPTANCE.md)."
        ) {
            return Err(CommandFailure::Unexpected(format!(
                "could not write status: {error}"
            )));
        }
    }

    // Select the profile and emit its banner BEFORE any device/output/
    // recorder/countdown/grab side effect (M11_TASK.md §11). Selection is
    // pure; an unknown name (unreachable through the validated CLI) or a
    // failed documented constant fails before anything is opened.
    let feel = match inputs.feel_config_path {
        Some(path) => Some(crate::cmd::feel::read_config(path)?),
        None => None,
    };
    let settings = match inputs.settings_path {
        Some(path) => Some(crate::cmd::settings::read_settings(path)?),
        None => None,
    };
    // The production binary leaves `streaming_factory` unset. The desktop
    // backend is selected by the application composition root and is never
    // inferred from the core profile name. When KDE is selected, validate
    // any loaded gesture map before device/output side effects so unsupported
    // actions fail before EVIOCGRAB.
    let real_desktop_plan = if env.takeover.streaming_factory.is_none() {
        Some(
            RealDesktopPlan::build(env.takeover.real_desktop_backend, settings.as_ref()).map_err(
                |error| {
                    CommandFailure::OutputCapability(format!(
                        "settings are not executable on the selected desktop output: {error}"
                    ))
                },
            )?,
        )
    } else {
        None
    };
    let selected = match select_profile_with_overlays(profile_name, feel, settings) {
        Ok(selected) => selected,
        Err(error) => {
            return Err(CommandFailure::Unexpected(format!(
                "could not select the takeover profile: {error}"
            )));
        }
    };
    let dwt_config = selected
        .arbiter_config
        .robustness_config()
        .map(|robustness| robustness.dwt_config().clone());
    let mut settings_watcher = if inputs.watch_settings {
        let path = inputs.settings_path.ok_or_else(|| {
            CommandFailure::Unexpected(
                "M19 --watch-settings reached runtime without --settings".to_string(),
            )
        })?;
        Some(crate::cmd::live_settings::SettingsWatcher::new(path)?)
    } else {
        None
    };
    if let Err(error) = writeln!(env.err, "{}", selected.banner) {
        return Err(CommandFailure::Unexpected(format!(
            "could not write status: {error}"
        )));
    }
    let resolved_device = match device {
        Some(device) => device.to_path_buf(),
        None => discover_unique_touchpad(env)?,
    };

    // Step 2: create the streaming session object (construction is **pure
    // object allocation** — side-effect-free; the real factory defers the
    // session-bus connection and the libei dlopen into `prepare`, M10 review
    // R6) and the M7–M9 arbiter pipeline (step 4: the bridge is the
    // decoder's sink, built before the device open because the decoder owns
    // it; construction is side-effect-free), then open the device (no read,
    // no grab). A device-open failure therefore performs **zero**
    // D-Bus/libei/output access and keeps its device-error precedence.
    let output = match env.takeover.streaming_factory.as_mut() {
        Some(factory) => factory().map_err(|error| {
            CommandFailure::OutputCapability(format!(
                "could not create the streaming output session: {error}"
            ))
        })?,
        None => real_desktop_plan
            .as_ref()
            .ok_or_else(|| {
                CommandFailure::Unexpected(
                    "real desktop output selected without a backend plan".to_string(),
                )
            })?
            .create_output()
            .map_err(|error| {
                CommandFailure::OutputCapability(format!(
                    "could not create the selected desktop output session: {error}"
                ))
            })?,
    };
    let bridge = TakeoverBridge::new(selected.arbiter_config, output);
    let mut runtime =
        EvdevRuntime::open(Rc::clone(&env.sys), &resolved_device, bridge).map_err(open_failure)?;
    runtime.set_stop_flag(std::sync::Arc::clone(&env.stop_flag));

    let mut guard = TakeoverCleanup {
        runtime: Some(runtime),
        keyboards: Vec::new(),
        unattached_recorder: None,
    };

    if dwt_config.is_some() {
        let touchpad_id = guard
            .runtime
            .as_ref()
            .expect("fresh runtime exists")
            .input_id();
        match discover_keyboards(&*env.sys, &resolved_device, touchpad_id) {
            Ok(candidates) => {
                for candidate in candidates {
                    match KeyboardMonitor::open(Rc::clone(&env.sys), &candidate) {
                        Ok(monitor) => {
                            writeln!(
                                env.err,
                                "DWT keyboard: {} ({}, read-only; no EVIOCGRAB)",
                                candidate.path.display(),
                                candidate.name
                            )
                            .map_err(|error| {
                                CommandFailure::Unexpected(format!(
                                    "could not write DWT status: {error}"
                                ))
                            })?;
                            guard.keyboards.push(monitor);
                        }
                        Err(error) => {
                            writeln!(env.err, "DWT keyboard skipped: {error}").map_err(
                                |write_error| {
                                    CommandFailure::Unexpected(format!(
                                        "could not write DWT status: {write_error}"
                                    ))
                                },
                            )?;
                        }
                    }
                }
                if guard.keyboards.is_empty() {
                    writeln!(
                        env.err,
                        "DWT unavailable: no paired internal typing keyboard found; touchpad remains usable"
                    )
                    .map_err(|error| {
                        CommandFailure::Unexpected(format!("could not write DWT status: {error}"))
                    })?;
                }
            }
            Err(error) => {
                writeln!(env.err, "DWT unavailable: {error}; touchpad remains usable").map_err(
                    |write_error| {
                        CommandFailure::Unexpected(format!(
                            "could not write DWT status: {write_error}"
                        ))
                    },
                )?;
            }
        }
    }

    // Step 3: prepare the streaming output session (cancellable, bounded
    // exactly as M6). A failure/cancel here must not leave a live session or
    // an open device: the ordered finalize releases both with zero grabs.
    let cancelled =
        || env.stop_flag.load(Ordering::Relaxed) || touchpad_linux::termination_requested();
    let capabilities = {
        let runtime = guard.runtime.as_mut().expect("runtime present");
        let bridge = runtime.sink_mut().expect("bridge present");
        match bridge.sink_mut().prepare(&cancelled) {
            Ok(capabilities) => capabilities,
            Err(error) => return finalize(env, &mut guard, StopReason::Output(error)),
        }
    };

    // The streaming session must expose the full M10 output contract: relative
    // pointer, primary button, secondary button, and pixel-precise two-axis
    // scroll. A missing capability refuses **before** the recorder and the
    // grab (M10_TASK.md §9).
    let required = [
        (
            "relative pointer",
            capabilities.supports(Capability::RelativePointer),
        ),
        (
            "primary button",
            capabilities.supports(Capability::PrimaryButton),
        ),
        (
            "secondary button",
            capabilities.supports(Capability::SecondaryButton),
        ),
        (
            "pixel-precise scroll",
            capabilities.supports(Capability::PixelScroll),
        ),
    ];
    if let Some((name, _)) = required.iter().find(|(_, ok)| !ok) {
        return finalize(
            env,
            &mut guard,
            StopReason::Output(DesktopOutputError::CapabilityMissing(format!(
                "the negotiated output session does not provide {name}; takeover is refused before the recorder or the grab"
            ))),
        );
    }

    // Developer takeover keeps its mandatory raw trace. Persistent service
    // mode deliberately does not record an unbounded touch stream by default;
    // users can run the explicit `record`/`takeover` tools when reproduction
    // evidence is required.
    if let Some(trace) = trace {
        let descriptor = guard
            .runtime
            .as_ref()
            .expect("runtime present")
            .descriptor()
            .cloned()
            .expect("a freshly opened runtime always exposes its descriptor");
        let recorder = match create_recorder(env, trace, &TraceHeader::new(descriptor)) {
            Ok(recorder) => recorder,
            Err(error) => return finalize(env, &mut guard, StopReason::RecorderPreflight(error)),
        };
        guard.unattached_recorder = Some(recorder);
        let flush_failed = {
            let recorder = guard
                .unattached_recorder
                .as_mut()
                .expect("recorder present");
            recorder.flush()
        };
        if let Err(error) = flush_failed {
            return finalize(env, &mut guard, StopReason::RecorderPreflight(error));
        }
        let recorder = guard.unattached_recorder.take().expect("recorder present");
        guard
            .runtime
            .as_mut()
            .expect("runtime present")
            .set_recorder(recorder);
    }

    // Step 6: print the resolved plan and run the visible cancellable
    // countdown (≥ 3 s; the sleeper is injectable so tests never sleep).
    let fd = guard
        .runtime
        .as_ref()
        .expect("runtime present")
        .fd()
        .expect("the device is open");
    status!(env, guard, "device: {}", resolved_device.display());
    if let Some(trace) = trace {
        status!(env, guard, "trace: {}", trace.display());
    } else {
        status!(env, guard, "trace: disabled in persistent service mode");
    }
    status!(
        env,
        guard,
        "profile: {} ({})",
        selected.name,
        selected.description
    );
    status!(
        env,
        guard,
        "negotiated capabilities: {}",
        capabilities.summary()
    );
    if let Some(max_duration_seconds) = max_duration_seconds {
        status!(env, guard, "maximum duration: {max_duration_seconds} seconds (bounded; the grab may exceed it by at most the {POLL_QUANTUM:?} polling quantum)");
    } else {
        status!(
            env,
            guard,
            "maximum duration: persistent until SIGINT/SIGTERM or runtime fault"
        );
    }
    status!(
        env,
        guard,
        "cleanup order on any stop: output release → recorder finalize → ungrab → close"
    );
    status!(env, guard, "escape routes: external keyboard/mouse; in a second terminal run `kill -TERM <pid>` (pid {})", std::process::id());
    if interactive {
        for remaining in (1..=COUNTDOWN_SECONDS).rev() {
            if cancelled() {
                return finalize(env, &mut guard, StopReason::CancelledBeforeGrab);
            }
            status!(
                env,
                guard,
                "takeover in {remaining} second(s)... (Ctrl-C to cancel)"
            );
            (env.takeover.sleeper)(Duration::from_secs(1));
        }
    }
    if cancelled() {
        return finalize(env, &mut guard, StopReason::CancelledBeforeGrab);
    }

    // Step 7: re-check every readiness state, then issue exactly one
    // EVIOCGRAB(1), immediately before the bounded event loop.
    if cancelled() {
        return finalize(env, &mut guard, StopReason::CancelledBeforeGrab);
    }
    if let Err(error) = guard.runtime.as_mut().expect("runtime present").grab() {
        return finalize(env, &mut guard, StopReason::GrabFailed(error));
    }

    let stop = run_loop(
        env,
        &mut guard,
        fd,
        max_duration_seconds,
        settings_watcher.as_mut(),
        real_desktop_plan.as_ref(),
    );
    finalize(env, &mut guard, stop)
}

/// The truly bounded event loop (M10_TASK.md §7).
///
/// Wakes at most every [`POLL_QUANTUM`]: checks the signal stop, the
/// injected monotonic clock (deadline), and the bridge fault, then reads
/// only when the readiness seam says the fd is ready. The maximum duration
/// therefore expires even when the touchpad produces no input, and the grab
/// exceeds the limit by at most the polling quantum (the deadline is checked
/// before every poll).
fn run_loop(
    env: &mut CommandEnv<'_>,
    guard: &mut TakeoverCleanup,
    fd: Fd,
    max_duration_seconds: Option<u32>,
    mut settings_watcher: Option<&mut crate::cmd::live_settings::SettingsWatcher>,
    real_desktop_plan: Option<&RealDesktopPlan>,
) -> StopReason {
    let start = (env.takeover.clock)();
    let mut tick_clock = InputDomainTickClock::default();
    let deadline = max_duration_seconds.map(|seconds| {
        start
            .checked_add(Duration::from_secs(u64::from(seconds)))
            .unwrap_or(Monotonic::from_nanos(u64::MAX))
    });
    let cancelled =
        || env.stop_flag.load(Ordering::Relaxed) || touchpad_linux::termination_requested();
    loop {
        if cancelled() {
            return StopReason::Signal;
        }
        let before_wait = (env.takeover.clock)();
        if deadline.is_some_and(|deadline| before_wait >= deadline) {
            return StopReason::Deadline;
        }
        // Inspect the bridge fault before the step (defensive) and after
        // every step (required): the first arbiter/output failure stops the
        // session; later frames from the same read batch are already ignored
        // by the bridge (the no-late-output rule).
        if let Some(fault) = take_bridge_fault(guard) {
            return StopReason::OutputFault(fault);
        }
        if let Some(watcher) = settings_watcher.as_deref_mut() {
            let pending_applied = guard
                .runtime
                .as_mut()
                .and_then(|runtime| runtime.sink_mut())
                .and_then(|bridge| watcher.try_apply_pending(bridge));
            if let Some(generation) = pending_applied {
                if let Err(error) = writeln!(
                    env.err,
                    "M19 settings reload applied generation {generation} at neutral boundary"
                ) {
                    return StopReason::StatusOutput(error);
                }
            }

            let reload = if let Some(plan) = real_desktop_plan {
                watcher.poll_validated(|settings| plan.validate_reload(settings))
            } else {
                watcher.poll()
            };
            match reload {
                crate::cmd::live_settings::ReloadPoll::Unchanged => {}
                crate::cmd::live_settings::ReloadPoll::Rejected(message) => {
                    if let Err(error) = writeln!(
                        env.err,
                        "M19 settings reload rejected: {message}; keeping last-good configuration"
                    ) {
                        return StopReason::StatusOutput(error);
                    }
                }
                crate::cmd::live_settings::ReloadPoll::Loaded { config, generation } => {
                    let applied = guard
                        .runtime
                        .as_mut()
                        .and_then(|runtime| runtime.sink_mut())
                        .is_some_and(|bridge| bridge.try_replace_config((*config).clone()));
                    if applied {
                        if let Err(error) = writeln!(
                            env.err,
                            "M19 settings reload applied generation {generation}"
                        ) {
                            return StopReason::StatusOutput(error);
                        }
                    } else {
                        watcher.queue(*config, generation);
                        if let Err(error) = writeln!(
                            env.err,
                            "M19 settings reload queued generation {generation}; waiting for neutral interaction boundary"
                        ) {
                            return StopReason::StatusOutput(error);
                        }
                    }
                }
            }
        }
        let momentum_active = guard
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.sink_mut())
            .is_some_and(|bridge| bridge.arbiter().is_scroll_momentum_active());
        let wait_quantum = if momentum_active {
            MOMENTUM_POLL_QUANTUM
        } else {
            POLL_QUANTUM
        };
        let ready = match (env.takeover.readiness)(fd, wait_quantum) {
            Ok(ready) => ready,
            Err(SysError::Interrupted) => {
                // R1 (M10 review): the installed SIGINT/SIGTERM handler runs
                // without `SA_RESTART`, so a normal Ctrl-C/SIGTERM while the
                // loop is idle interrupts `poll(2)` and surfaces as EINTR.
                // Re-check both stop sources: a requested stop is the
                // documented controlled stop (clean, exit 0); an unrequested
                // EINTR keeps its M4/M5 semantics as an actionable poll
                // failure (stream error, exit 6) — never misclassified.
                if cancelled() {
                    return StopReason::Signal;
                }
                return StopReason::Stream(RuntimeError::Read(SysError::Interrupted));
            }
            Err(error) => return StopReason::Stream(RuntimeError::Read(error)),
        };
        let mut keyboard_index = 0;
        while keyboard_index < guard.keyboards.len() {
            let keyboard_ready = guard.keyboards[keyboard_index]
                .fd()
                .and_then(|keyboard_fd| env.sys.poll(keyboard_fd, Duration::ZERO).ok())
                .unwrap_or(false);
            if !keyboard_ready {
                keyboard_index += 1;
                continue;
            }
            match guard.keyboards[keyboard_index].read_activity() {
                Ok(activity) => {
                    if let Some(bridge) = guard
                        .runtime
                        .as_mut()
                        .and_then(|runtime| runtime.sink_mut())
                    {
                        for timestamp in activity {
                            bridge.note_typing(timestamp);
                        }
                    }
                    keyboard_index += 1;
                }
                Err(error) => {
                    if let Err(write_error) = writeln!(
                        env.err,
                        "DWT keyboard removed after read failure: {error}; continuing without it"
                    ) {
                        return StopReason::StatusOutput(write_error);
                    }
                    guard.keyboards.remove(keyboard_index);
                }
            }
        }
        if ready {
            match guard
                .runtime
                .as_mut()
                .expect("runtime present")
                .step_deferred()
            {
                Ok(_) => {}
                Err(RuntimeError::Interrupted) => return StopReason::Signal,
                Err(error) => return StopReason::Stream(error),
            }
            let observed_at = (env.takeover.clock)();
            let input_marker = guard
                .runtime
                .as_mut()
                .and_then(|runtime| runtime.sink_mut())
                .map(|bridge| {
                    (
                        bridge.arbiter().last_input_sequence(),
                        bridge.arbiter().last_input_timestamp(),
                    )
                });
            if let Some((input_sequence, input_timestamp)) = input_marker {
                tick_clock.observe_input(observed_at, input_sequence, input_timestamp);
            }
        }
        // Policy timers are time-driven rather than input-driven. Advance
        // them after the readiness wait. Today this primarily commits the
        // delayed ButtonUp for libinput-style tap-and-drag; kinetic scrolling
        // no longer lives in core. The bounded-deadline clock is mapped onto
        // the latest accepted input-frame epoch before it enters core.
        let after_wait = (env.takeover.clock)();
        if guard
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.sink_mut())
            .is_some_and(|bridge| bridge.arbiter().needs_timer_tick())
        {
            if let Some(bridge) = guard
                .runtime
                .as_mut()
                .and_then(|runtime| runtime.sink_mut())
            {
                if let Some(input_domain_now) = tick_clock.map_process_now(after_wait) {
                    let _ = bridge.tick(input_domain_now);
                }
            }
        }
        if let Some(fault) = take_bridge_fault(guard) {
            return StopReason::OutputFault(fault);
        }
    }
}

/// Takes the bridge's stored fault out of the runtime, if any.
fn take_bridge_fault(guard: &mut TakeoverCleanup) -> Option<ArbiterSinkError> {
    guard
        .runtime
        .as_mut()
        .and_then(|runtime| runtime.sink_mut())
        .and_then(TakeoverBridge::take_fault)
}

/// The unified ordered shutdown (M10_TASK.md §8). Idempotent: repeated calls
/// are safe no-ops because the guard is emptied by the first call.
///
/// Exit-code precedence (deterministic, documented in the help text):
///
/// ```text
/// recorder finalization failure  → 7 (RecorderFinalize)
/// output release failure         → 7 (OutputReleaseFailed)
/// device release failure         → 6 (DeviceRelease)
/// status-output failure          → 9 (Unexpected)
/// primary stop reason            → deadline/signal: 0 (clean, only when all
///                                  cleanup succeeded)
///                                  countdown cancel: 8 (TakeoverAborted)
///                                  stream/grab/output fault: 6
///                                  output prepare: 2/3/4/5 by category
///                                  recorder preflight: 7
/// ```
///
/// The returned message preserves the primary reason and **every** cleanup
/// failure.
fn finalize(
    env: &mut CommandEnv<'_>,
    guard: &mut TakeoverCleanup,
    primary: StopReason,
) -> Result<(), CommandFailure> {
    // 1. Output release: take the bridge (the decoder's sink) out of the
    //    runtime and release the owed virtual Left/Right and scroll
    //    lifecycle; the wrapped streaming session's own release then
    //    disconnects the transport and closes the portal session. This runs
    //    BEFORE the recorder finalization and the device release.
    let (bridge, frames_processed, frames_ignored) =
        match guard.runtime.as_mut().and_then(EvdevRuntime::take_sink) {
            Some(bridge) => {
                let processed = bridge.frames_processed();
                let ignored = bridge.frames_ignored_after_fault();
                (Some(bridge), processed, ignored)
            }
            None => (None, 0, 0),
        };
    let mut server_interruption = None;
    let mut sink_cleanup_error = None;
    let output_release = match bridge {
        Some(mut bridge) => {
            // R3 (M10 review): capture the structured server-side
            // interruption (device pause/removal, seat removal, disconnect)
            // observed by the streaming session BEFORE `release_all` runs —
            // the real `PortalOutputSink::release_all_detailed` clears its
            // interruption during the release, so reading it afterwards
            // would flatten a real interruption into a generic
            // semantic-output failure. It keeps its structured category
            // (M6-consistent exit) for an output-fault primary (see
            // [`compose`]).
            server_interruption = bridge.sink_mut().take_server_interruption();
            let result = bridge.release_all();
            // Consume the session-level cleanup error (the `StreamingOutput`
            // accessor must not stay dead). The arbiter-level release
            // failure — `ArbiterSinkError::ReleaseFailed { cleanup, .. }` —
            // already carries the wrapped sink's cleanup error when the
            // wrapped release failed, so it is only surfaced separately here
            // when the arbiter-level release succeeded (a successful wrapped
            // release clears the sink's own cleanup error, so this is
            // normally absent) — the diagnostic is preserved, never
            // duplicated (M10 review R3).
            let (_, mut session) = bridge.into_parts();
            sink_cleanup_error = session.take_cleanup_error();
            result
        }
        None => Ok(()),
    };

    // 2. Unattached recorder finalize (created but never attached: header
    //    flush failed). The best-effort Drop flush also runs here, before the
    //    device release.
    let mut unattached_finish = None;
    if let Some(mut recorder) = guard.unattached_recorder.take() {
        unattached_finish = Some(recorder.finish());
        drop(recorder);
    }

    // 3. Close read-only keyboard listeners. They were never grabbed, so
    //    this is only fd cleanup and does not alter keyboard delivery.
    guard.keyboards.clear();

    // 4. Runtime ordered shutdown: attached recorder finalize → EVIOCGRAB(0)
    //    at most once → close the fd exactly once (idempotent; pre-grab
    //    failures issue zero ungrab ioctls).
    let report = match guard.runtime.take() {
        Some(mut runtime) => runtime.shutdown(),
        None => ShutdownReport {
            phase: touchpad_linux::RuntimePhase::Stopped,
            recorder_finish: None,
            events_recorded: 0,
            ungrab: None,
            close: None,
        },
    };

    // 5. Status output: every step's actual result printed from the same
    //    source as the exit decision; a status-write failure is recorded and
    //    reported (never silently dropped).
    let mut status_failure: Option<std::io::Error> = None;
    let mut print = |line: &str| {
        if status_failure.is_none() {
            if let Err(error) = writeln!(env.err, "{line}") {
                status_failure = Some(error);
            }
        }
    };
    print(&format!("takeover stopped: {}", stop_reason_text(&primary)));
    print(&format!(
        "frames: {frames_processed} processed, {frames_ignored} ignored after a fault"
    ));
    print(&format!(
        "cleanup: output release {}, recorder {}, ungrab {}, close {}",
        ok_err(&output_release),
        recorder_status(&unattached_finish, &report),
        ok_err_opt(&report.ungrab),
        ok_err_opt(&report.close),
    ));

    compose(
        primary,
        server_interruption,
        output_release,
        sink_cleanup_error,
        unattached_finish,
        report,
        status_failure,
    )
}

/// Builds the recorder status text from the unattached and attached
/// recorder finalization results.
fn recorder_status(
    unattached: &Option<Result<(), RecorderError>>,
    report: &ShutdownReport,
) -> String {
    match (&unattached, &report.recorder_finish) {
        (Some(result), _) => ok_err(result),
        (None, Some(result)) => ok_err(result),
        (None, None) => "n/a (no recorder)".to_string(),
    }
}

/// Maps a `Result` to `ok` / `error (...)` text.
fn ok_err<T, E: std::fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => format!("error ({error})"),
    }
}

/// Maps an optional `Result` to `ok` / `error (...)` / `n/a`.
fn ok_err_opt<T, E: std::fmt::Display>(result: &Option<Result<T, E>>) -> String {
    match result {
        Some(result) => ok_err(result),
        None => "n/a".to_string(),
    }
}

/// The human-readable primary stop reason.
fn stop_reason_text(reason: &StopReason) -> String {
    match reason {
        StopReason::Signal => "SIGINT/SIGTERM (controlled stop)".to_string(),
        StopReason::Deadline => "maximum duration reached (bounded takeover)".to_string(),
        StopReason::CancelledBeforeGrab => {
            "aborted by the user before the takeover began; nothing was grabbed and no desktop input was emitted"
                .to_string()
        }
        StopReason::Stream(error) => error.to_string(),
        StopReason::GrabFailed(error) => error.to_string(),
        StopReason::OutputFault(error) => error.to_string(),
        StopReason::Output(error) => error.to_string(),
        StopReason::RecorderPreflight(error) => error.to_string(),
        StopReason::StatusOutput(error) => format!("status output failed: {error}"),
    }
}

/// Composes the final [`CommandFailure`] from the primary stop reason and
/// every cleanup result, following the documented precedence.
fn compose(
    primary: StopReason,
    server_interruption: Option<DesktopOutputError>,
    output_release: Result<(), ArbiterSinkError>,
    sink_cleanup_error: Option<DesktopOutputError>,
    unattached_finish: Option<Result<(), RecorderError>>,
    report: ShutdownReport,
    status_failure: Option<std::io::Error>,
) -> Result<(), CommandFailure> {
    let recorder_failed = unattached_finish
        .as_ref()
        .map(Result::is_err)
        .unwrap_or(false)
        || report
            .recorder_finish
            .as_ref()
            .map(Result::is_err)
            .unwrap_or(false);
    let output_release_failed = output_release.is_err();
    let device_release_failed = report.ungrab.as_ref().map(Result::is_err).unwrap_or(false)
        || report.close.as_ref().map(Result::is_err).unwrap_or(false);

    // The full message: the primary reason first, then every cleanup failure.
    let mut parts = vec![stop_reason_text(&primary)];
    if let Err(error) = &output_release {
        parts.push(format!("output release failed: {error}"));
    } else if let Some(error) = &sink_cleanup_error {
        // The arbiter-level release succeeded, so the wrapped sink's own
        // cleanup error is not carried by the `ArbiterSinkError` — surface it
        // separately so no diagnostic is lost (a successful wrapped release
        // clears the sink's cleanup error, so this is normally absent).
        parts.push(format!("wrapped output cleanup failed: {error}"));
    }
    if let Some(Err(error)) = &unattached_finish {
        parts.push(format!("recorder finalize failed: {error}"));
    }
    if let Some(Err(error)) = &report.recorder_finish {
        parts.push(format!("recorder finish failed: {error}"));
    }
    if let Some(Err(error)) = &report.ungrab {
        parts.push(format!("ungrab failed: {error}"));
    }
    if let Some(Err(error)) = &report.close {
        parts.push(format!("close failed: {error}"));
    }
    if let Some(error) = &status_failure {
        parts.push(format!("status output failed: {error}"));
    }
    let message = parts.join("; ");

    // Precedence: recorder finalize (7) > output release (7) > device release
    // (6) > status output (9) > primary stop reason.
    if recorder_failed {
        return Err(CommandFailure::RecorderFinalize(message));
    }
    if output_release_failed {
        return Err(CommandFailure::OutputReleaseFailed(message));
    }
    if device_release_failed {
        return Err(CommandFailure::DeviceRelease(message));
    }
    if status_failure.is_some() {
        return Err(CommandFailure::Unexpected(message));
    }
    match primary {
        // A controlled deadline/signal stop is clean (exit 0) only when all
        // required cleanup succeeded — checked above.
        StopReason::Deadline | StopReason::Signal => Ok(()),
        StopReason::CancelledBeforeGrab => Err(CommandFailure::TakeoverAborted(message)),
        StopReason::Stream(error) | StopReason::GrabFailed(error) => Err(stream_failure(error)),
        StopReason::OutputFault(error) => {
            // A server-side interruption (device pause/removal, seat removal,
            // disconnect) observed by the streaming session keeps the
            // M6-consistent transport exit code (5); any other output
            // rejection is a stream failure (6). The structured fault is
            // preserved in the message either way.
            if let Some(interruption) = server_interruption {
                Err(crate::cmd::output_probe::output_probe_failure(interruption))
            } else {
                Err(CommandFailure::Stream(format!(
                    "semantic output fault: {error}"
                )))
            }
        }
        StopReason::Output(error) => Err(crate::cmd::output_probe::output_probe_failure(error)),
        StopReason::RecorderPreflight(error) => Err(CommandFailure::Recorder(error)),
        StopReason::StatusOutput(error) => Err(CommandFailure::Unexpected(format!(
            "status output failed: {error}"
        ))),
    }
}

/// Maps a fatal stream error to a [`CommandFailure`] (exit 6), except
/// recorder event errors (exit 7).
fn stream_failure(error: RuntimeError) -> CommandFailure {
    match error {
        RuntimeError::Recorder(error) => CommandFailure::Recorder(error),
        other => CommandFailure::Stream(other.to_string()),
    }
}

/// Builds the raw-event recorder for the session: the env's injected factory
/// (fault-injection / timeline tests) or the real
/// [`touchpad_linux::TraceRecorder::create`].
fn create_recorder(
    env: &CommandEnv<'_>,
    trace: &Path,
    header: &TraceHeader,
) -> Result<Box<dyn RawEventRecorder>, RecorderError> {
    match &env.recorder_factory {
        Some(factory) => factory(trace, header),
        None => Ok(Box::new(touchpad_linux::TraceRecorder::create(
            trace, header,
        )?)),
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
        other => CommandFailure::Stream(format!("could not open the device: {other}")),
    }
}

#[cfg(test)]
mod tests;
