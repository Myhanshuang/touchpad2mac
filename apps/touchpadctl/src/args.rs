//! Command-line argument parsing and help text (hand-rolled, dependency-free,
//! fully unit-tested).

use std::path::PathBuf;

/// A parsed top-level command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the top-level help (exit 0).
    Help,
    /// `devices` — enumerate and explain every `/dev/input/event*` node.
    Devices,
    /// `inspect DEVICE` — probe one device node.
    Inspect {
        /// The device node to probe.
        device: PathBuf,
    },
    /// `record DEVICE OUTPUT [--grab]` — record raw events into a trace.
    Record {
        /// The device node to read.
        device: PathBuf,
        /// The trace output file.
        output: PathBuf,
        /// Whether to grab the device exclusively (`EVIOCGRAB(1)`). Off by
        /// default; `--grab` is an explicit opt-in with documented risks.
        grab: bool,
    },
    /// `replay INPUT` — offline replay of a raw trace.
    Replay {
        /// The trace input file.
        input: PathBuf,
    },
    /// `output-probe [--emit]` — probe the KDE Wayland output backend
    /// (portal + libei). The default is a non-emitting dry-run; `--emit` is
    /// an explicit opt-in that runs a short, fixed, bounded test pattern on
    /// the real desktop (M6).
    OutputProbe {
        /// Whether to run the real `--emit` pattern (explicit opt-in).
        emit: bool,
    },
    /// `config-check FILE` — strict offline M16 runtime configuration
    /// validation/migration. Reads no device and starts no service.
    ConfigCheck {
        /// JSON configuration file.
        input: PathBuf,
    },
    /// `service-preflight FILE` — offline/foreground-only M16 preparation
    /// report. It does not install or start a service.
    ServicePreflight {
        /// JSON configuration file.
        input: PathBuf,
    },
    /// Writes the built-in M17 default feel overlay as pretty JSON.
    FeelDefault {
        /// Destination JSON file.
        output: PathBuf,
    },
    /// Strictly validates one M17 feel overlay.
    FeelCheck {
        /// Feel JSON file to validate.
        input: PathBuf,
    },
    /// Prints one validated M17 feel overlay as pretty JSON.
    FeelShow {
        /// Feel JSON file to normalize/print.
        input: PathBuf,
    },
    /// Applies one or more `key=value` edits to a validated M17 feel overlay.
    FeelSet {
        /// Existing validated feel JSON.
        input: PathBuf,
        /// Destination for the edited validated JSON.
        output: PathBuf,
        /// One or more `key=value` edits.
        edits: Vec<String>,
    },
    /// Generates a self-contained offline HTML editor for one feel overlay.
    FeelGui {
        /// Existing validated feel JSON used as initial values.
        input: PathBuf,
        /// Destination self-contained HTML file.
        output: PathBuf,
    },
    /// Writes the built-in M18 default unified settings document.
    SettingsDefault {
        /// Destination JSON file.
        output: PathBuf,
    },
    /// Writes the built-in macOS-inspired M18 settings preset.
    SettingsMacos {
        /// Destination JSON file.
        output: PathBuf,
    },
    /// Strictly validates one M18 unified settings document.
    SettingsCheck {
        /// Settings JSON file.
        input: PathBuf,
    },
    /// Prints one validated M18 settings document as normalized JSON.
    SettingsShow {
        /// Settings JSON file.
        input: PathBuf,
    },
    /// Applies feel/gesture `key=value` edits to a settings document.
    SettingsSet {
        /// Existing validated settings JSON.
        input: PathBuf,
        /// Destination JSON file.
        output: PathBuf,
        /// One or more settings edits.
        edits: Vec<String>,
    },
    /// Applies validated settings edits in-place for a running M19 watcher.
    SettingsPatch {
        /// Existing settings JSON file to update in-place.
        input: PathBuf,
        /// One or more settings edits.
        edits: Vec<String>,
    },
    /// Generates a self-contained offline M18 settings editor.
    SettingsGui {
        /// Existing validated settings JSON.
        input: PathBuf,
        /// Destination HTML file.
        output: PathBuf,
    },
    /// `takeover TRACE [--device DEVICE] --takeover --confirm TAKEOVER
    /// --output-qualified --profile m10-linear-v1 --max-duration-seconds N`
    /// — the M10 bounded, fail-open live takeover slice: exclusively grab
    /// `DEVICE`, stream the decoded contacts through the approved M7–M9
    /// arbiter pipeline (`m10-linear-v1`), emit the resolved semantic events
    /// through a prepared portal/libei streaming session, and record the raw
    /// input to `TRACE` for at most `N` seconds (1..=300). Foreground-only;
    /// every opt-in is mandatory and independently validated (M10_TASK.md
    /// §2/§4).
    Takeover {
        /// The physical device node to take over. `None` means discover the
        /// unique touchpad candidate at runtime. The legacy positional
        /// `takeover DEVICE TRACE ...` spelling remains accepted.
        device: Option<PathBuf>,
        /// The mandatory raw-event trace output.
        trace: PathBuf,
        /// The maximum duration in seconds (`1..=300`; no zero, overflow,
        /// missing, repeated, or unlimited form).
        max_duration_seconds: u32,
        /// The validated versioned policy profile name (`m10-linear-v1`).
        profile: String,
        /// M17-only explicit feel overlay. Required exactly for
        /// `m17-tunable-v1`; forbidden for every earlier profile.
        feel_config: Option<PathBuf>,
        /// M18-only unified user settings. Required exactly for
        /// `m18-remap-v1`; forbidden for all earlier profiles.
        settings: Option<PathBuf>,
        /// M19-only explicit settings hot-reload opt-in.
        watch_settings: bool,
    },
}

/// A usage/argument error (exit code 1).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    /// No command was given.
    #[error("no command given (try `touchpadctl --help`)")]
    NoCommand,
    /// An unknown subcommand.
    #[error("unknown command {0:?} (try `touchpadctl --help`)")]
    UnknownCommand(String),
    /// An unknown flag.
    #[error("unknown flag {0:?} (try `touchpadctl --help`)")]
    UnknownFlag(String),
    /// `--grab` was passed to a command other than `record` (M5 review R5):
    /// the flag is record-only, so `devices --grab`, `inspect DEVICE
    /// --grab`, and `replay INPUT --grab` are usage errors before any work.
    #[error(
        "--grab is only valid with `record` (got command {command:?}); try `touchpadctl --help`"
    )]
    GrabNotAllowed {
        /// The command that received `--grab`.
        command: &'static str,
    },
    /// `--grab` was repeated (M5 review R5: duplicates are rejected rather
    /// than silently accepted).
    #[error("--grab may only be given once (got {count}); try `touchpadctl --help`")]
    DuplicateGrab {
        /// How many times `--grab` appeared.
        count: usize,
    },
    /// `--emit` was repeated (M6: the output-probe emission opt-in is
    /// explicit and may only be given once).
    #[error("--emit may only be given once (got {count}); try `touchpadctl --help`")]
    DuplicateEmit {
        /// How many times `--emit` appeared.
        count: usize,
    },
    /// A command received the wrong number of positional arguments.
    #[error(
        "command {command} expects {expected} argument(s), got {actual} (try `touchpadctl --help`)"
    )]
    WrongArity {
        /// The command name.
        command: &'static str,
        /// The expected positional argument count.
        expected: usize,
        /// The actual positional argument count.
        actual: usize,
    },
    // ------------------------------------------------------------------
    // M10 takeover usage errors (M10_TASK.md §2: every opt-in is mandatory
    // and independently validated; unknown/repeated flags and takeover-only
    // flags on other commands are usage errors before any side effect).
    // ------------------------------------------------------------------
    /// `--takeover` was not given to the `takeover` command.
    #[error("`takeover` requires `--takeover` (try `touchpadctl --help`)")]
    TakeoverFlagRequired,
    /// `--confirm TAKEOVER` was not given to the `takeover` command.
    #[error("`takeover` requires `--confirm TAKEOVER` (try `touchpadctl --help`)")]
    ConfirmRequired,
    /// `--confirm` was given without its value.
    #[error(
        "`--confirm` requires the exact confirmation text TAKEOVER (try `touchpadctl --help`)"
    )]
    ConfirmNeedsValue,
    /// The `--confirm` value was not the exact confirmation text.
    #[error(
        "the confirmation text must be exactly TAKEOVER, got {found:?} (try `touchpadctl --help`)"
    )]
    ConfirmTextMismatch {
        /// The value that was given instead of `TAKEOVER`.
        found: String,
    },
    /// `--output-qualified` was not given to the `takeover` command.
    #[error(
        "`takeover` requires `--output-qualified` (the operator attestation that the M6 output calibration was performed; see doc/old/acceptance/M6_ACCEPTANCE.md §3 — the attestation is not itself measurement evidence)"
    )]
    OutputQualifiedRequired,
    /// `--profile` was not given to the `takeover` command.
    #[error("`takeover` requires `--profile` (try `touchpadctl --help`)")]
    ProfileRequired,
    /// `m17-tunable-v1` requires an explicit tuning overlay.
    #[error("`m17-tunable-v1` requires `--feel-config FILE`; no tuning file is inferred")]
    FeelConfigRequired,
    /// `--feel-config` is M17-only and cannot alter earlier profile behavior.
    #[error("`--feel-config` is only valid with `--profile m17-tunable-v1`, got {profile:?}")]
    FeelConfigOnlyM17 {
        /// Earlier profile that incorrectly received the flag.
        profile: String,
    },
    /// `m18-remap-v1` requires the unified settings document.
    #[error("`m18-remap-v1` requires `--settings FILE`; no settings file is inferred")]
    SettingsRequired,
    /// `--settings` is accepted only by M18/M19 user-settings profiles.
    #[error("`--settings` is only valid with `--profile m18-remap-v1` or `m19-live-v1`, got {profile:?}")]
    SettingsOnlyM18M19 {
        /// Profile that incorrectly received the flag.
        profile: String,
    },
    /// M19 live settings profile requires explicit hot-reload opt-in.
    #[error("`m19-live-v1` requires `--watch-settings`; live reload is never inferred")]
    WatchSettingsRequired,
    /// Hot reload is M19-only.
    #[error("`--watch-settings` is only valid with `--profile m19-live-v1`, got {profile:?}")]
    WatchSettingsOnlyM19 {
        /// Profile that incorrectly received the flag.
        profile: String,
    },
    /// `--profile` named an unknown profile.
    #[error(
        "unknown policy profile {found:?}; accepted profiles are `m10-linear-v1`, `m11-fidelity-v1`, `m12-scroll-v1`, `m13-robust-v1`, `m14-gestures-v1`, `m15-kde-v1`, `m16-production-v1`, `m17-tunable-v1`, `m18-remap-v1`, and `m19-live-v1` (try `touchpadctl --help`)"
    )]
    UnknownProfile {
        /// The profile name that was given.
        found: String,
    },
    /// `--max-duration-seconds` was not given to the `takeover` command.
    #[error("`takeover` requires `--max-duration-seconds N` (try `touchpadctl --help`)")]
    DurationRequired,
    /// `--max-duration-seconds` was missing its value.
    #[error(
        "`--max-duration-seconds` requires an integer number of seconds (try `touchpadctl --help`)"
    )]
    DurationNeedsValue,
    /// `--max-duration-seconds` was not an integer in `1..=300`.
    #[error(
        "maximum duration must be an integer in 1..=300 seconds, got {found:?} (no zero, overflow, or unlimited form is accepted; try `touchpadctl --help`)"
    )]
    DurationInvalid {
        /// The value that was given.
        found: String,
    },
    /// A takeover-only flag was repeated.
    #[error("flag {flag} may only be given once (try `touchpadctl --help`)")]
    DuplicateTakeoverFlag {
        /// The repeated flag.
        flag: &'static str,
    },
    /// `--device` was present without its device-node value.
    #[error("`--device` requires a device node such as /dev/input/event15")]
    DeviceNeedsValue,
    /// The device was supplied both through `--device` and through the
    /// legacy positional `DEVICE TRACE` spelling.
    #[error(
        "touchpad device specified twice; use `takeover TRACE --device DEVICE ...` or the legacy `takeover DEVICE TRACE ...`, not both"
    )]
    DeviceSpecifiedTwice,
    /// A takeover-only flag was passed to another command.
    #[error(
        "flag {flag:?} is only valid with `takeover` (got command {command:?}); try `touchpadctl --help`"
    )]
    TakeoverFlagNotAllowed {
        /// The command that received the flag.
        command: &'static str,
        /// The takeover-only flag.
        flag: String,
    },
}

/// The exact confirmation text `--confirm` must carry.
pub const TAKEOVER_CONFIRM_TEXT: &str = "TAKEOVER";

/// The mention-first baseline policy profile the takeover command accepts
/// (M10_TASK.md §3).
pub const TAKEOVER_PROFILE: &str = touchpad_core::m10::M10_LINEAR_V1_NAME;

/// Accepted policy profiles, ordered from the stable bring-up baseline toward
/// progressively newer experimental layers. No profile is inferred.
pub const ACCEPTED_TAKEOVER_PROFILES: &[&str] = &[
    TAKEOVER_PROFILE,
    touchpad_core::m11::M11_FIDELITY_V1_NAME,
    touchpad_core::m12::M12_SCROLL_V1_NAME,
    touchpad_core::m13::M13_ROBUST_V1_NAME,
    touchpad_core::m14::M14_GESTURES_V1_NAME,
    touchpad_core::m15::M15_KDE_V1_NAME,
    touchpad_core::m16::M16_PRODUCTION_V1_NAME,
    touchpad_core::m17::M17_TUNABLE_V1_NAME,
    touchpad_core::m18::M18_REMAP_V1_NAME,
    touchpad_core::m19::M19_LIVE_V1_NAME,
];

/// The maximum takeover duration in seconds (M10_TASK.md §2).
pub const MAX_TAKEOVER_SECONDS: u32 = 300;

/// The minimum takeover duration in seconds.
pub const MIN_TAKEOVER_SECONDS: u32 = 1;

/// The takeover-only flags, rejected by every other command.
const TAKEOVER_ONLY_FLAGS: &[&str] = &[
    "--takeover",
    "--confirm",
    "--output-qualified",
    "--device",
    "--profile",
    "--max-duration-seconds",
    "--feel-config",
    "--settings",
    "--watch-settings",
];

/// The known top-level command names as `'static` strings (the takeover-flag
/// rejection needs a `'static` command name for the diagnostic).
fn static_command_name(name: &str) -> Option<&'static str> {
    match name {
        "devices" => Some("devices"),
        "inspect" => Some("inspect"),
        "record" => Some("record"),
        "replay" => Some("replay"),
        "output-probe" => Some("output-probe"),
        "config-check" => Some("config-check"),
        "service-preflight" => Some("service-preflight"),
        "feel-default" => Some("feel-default"),
        "feel-check" => Some("feel-check"),
        "feel-show" => Some("feel-show"),
        "feel-set" => Some("feel-set"),
        "feel-gui" => Some("feel-gui"),
        "settings-default" => Some("settings-default"),
        "settings-macos" => Some("settings-macos"),
        "settings-check" => Some("settings-check"),
        "settings-show" => Some("settings-show"),
        "settings-set" => Some("settings-set"),
        "settings-patch" => Some("settings-patch"),
        "settings-gui" => Some("settings-gui"),
        _ => None,
    }
}

/// Parses the command line (excluding `argv[0]`).
///
/// `-h`/`--help` anywhere prints the top-level help. `record` accepts the
/// optional `--grab` flag (at most once); **every other command rejects
/// `--grab` as a usage error** (M5 review R5) and any other flag is a usage
/// error too.
pub fn parse_args<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let Some(first) = args.first() else {
        return Err(UsageError::NoCommand);
    };
    if first == "-h" || first == "--help" {
        return Ok(Command::Help);
    }
    if args
        .iter()
        .skip(1)
        .any(|arg| arg == "-h" || arg == "--help")
    {
        return Ok(Command::Help);
    }
    if first == "takeover" {
        return parse_takeover(&args[1..]);
    }
    let positional: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let flags: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|arg| arg.starts_with('-'))
        .collect();

    // M10 (M10_TASK.md §2): takeover-only flags are rejected on every other
    // command as usage errors **before** any arity/side effect, so
    // `replay t.jsonl --confirm TAKEOVER` is never misreported as an arity
    // error.
    if let Some(flag) = flags
        .iter()
        .find(|flag| TAKEOVER_ONLY_FLAGS.contains(&flag.as_str()))
    {
        if let Some(command) = static_command_name(first.as_str()) {
            return Err(UsageError::TakeoverFlagNotAllowed {
                command,
                flag: flag.to_string(),
            });
        }
    }

    match first.as_str() {
        "devices" => {
            check_arity("devices", 0, &positional)?;
            reject_non_record_flags("devices", &flags)?;
            Ok(Command::Devices)
        }
        "inspect" => {
            check_arity("inspect", 1, &positional)?;
            reject_non_record_flags("inspect", &flags)?;
            Ok(Command::Inspect {
                device: PathBuf::from(positional[0]),
            })
        }
        "record" => {
            check_arity("record", 2, &positional)?;
            // `--grab` is record-only and may appear at most once (M5 review
            // R5: duplicates are rejected).
            let grab_count = flags
                .iter()
                .filter(|flag| flag.as_str() == "--grab")
                .count();
            if grab_count > 1 {
                return Err(UsageError::DuplicateGrab { count: grab_count });
            }
            for flag in &flags {
                if flag.as_str() != "--grab" {
                    if TAKEOVER_ONLY_FLAGS.contains(&flag.as_str()) {
                        return Err(UsageError::TakeoverFlagNotAllowed {
                            command: "record",
                            flag: flag.to_string(),
                        });
                    }
                    return Err(UsageError::UnknownFlag(flag.to_string()));
                }
            }
            Ok(Command::Record {
                device: PathBuf::from(positional[0]),
                output: PathBuf::from(positional[1]),
                grab: grab_count == 1,
            })
        }
        "replay" => {
            check_arity("replay", 1, &positional)?;
            reject_non_record_flags("replay", &flags)?;
            Ok(Command::Replay {
                input: PathBuf::from(positional[0]),
            })
        }
        "output-probe" => {
            check_arity("output-probe", 0, &positional)?;
            // `--emit` is the M6 explicit opt-in for real desktop emission
            // and may appear at most once; `--grab` is rejected (record
            // only).
            let emit_count = flags
                .iter()
                .filter(|flag| flag.as_str() == "--emit")
                .count();
            if emit_count > 1 {
                return Err(UsageError::DuplicateEmit { count: emit_count });
            }
            for flag in &flags {
                if flag.as_str() == "--emit" {
                    continue;
                }
                if flag.as_str() == "--grab" {
                    return Err(UsageError::GrabNotAllowed {
                        command: "output-probe",
                    });
                }
                if TAKEOVER_ONLY_FLAGS.contains(&flag.as_str()) {
                    return Err(UsageError::TakeoverFlagNotAllowed {
                        command: "output-probe",
                        flag: flag.to_string(),
                    });
                }
                return Err(UsageError::UnknownFlag(flag.to_string()));
            }
            Ok(Command::OutputProbe {
                emit: emit_count == 1,
            })
        }
        "config-check" => {
            check_arity("config-check", 1, &positional)?;
            reject_non_record_flags("config-check", &flags)?;
            Ok(Command::ConfigCheck {
                input: PathBuf::from(positional[0]),
            })
        }
        "service-preflight" => {
            check_arity("service-preflight", 1, &positional)?;
            reject_non_record_flags("service-preflight", &flags)?;
            Ok(Command::ServicePreflight {
                input: PathBuf::from(positional[0]),
            })
        }
        "feel-default" => {
            check_arity("feel-default", 1, &positional)?;
            reject_non_record_flags("feel-default", &flags)?;
            Ok(Command::FeelDefault {
                output: PathBuf::from(positional[0]),
            })
        }
        "feel-check" => {
            check_arity("feel-check", 1, &positional)?;
            reject_non_record_flags("feel-check", &flags)?;
            Ok(Command::FeelCheck {
                input: PathBuf::from(positional[0]),
            })
        }
        "feel-show" => {
            check_arity("feel-show", 1, &positional)?;
            reject_non_record_flags("feel-show", &flags)?;
            Ok(Command::FeelShow {
                input: PathBuf::from(positional[0]),
            })
        }
        "feel-gui" => {
            check_arity("feel-gui", 2, &positional)?;
            reject_non_record_flags("feel-gui", &flags)?;
            Ok(Command::FeelGui {
                input: PathBuf::from(positional[0]),
                output: PathBuf::from(positional[1]),
            })
        }
        "feel-set" => {
            reject_non_record_flags("feel-set", &flags)?;
            if positional.len() < 3 {
                return Err(UsageError::WrongArity {
                    command: "feel-set",
                    expected: 3,
                    actual: positional.len(),
                });
            }
            Ok(Command::FeelSet {
                input: PathBuf::from(positional[0]),
                output: PathBuf::from(positional[1]),
                edits: positional[2..]
                    .iter()
                    .map(|value| (*value).clone())
                    .collect(),
            })
        }
        "settings-default" => {
            check_arity("settings-default", 1, &positional)?;
            reject_non_record_flags("settings-default", &flags)?;
            Ok(Command::SettingsDefault {
                output: PathBuf::from(positional[0]),
            })
        }
        "settings-macos" => {
            check_arity("settings-macos", 1, &positional)?;
            reject_non_record_flags("settings-macos", &flags)?;
            Ok(Command::SettingsMacos {
                output: PathBuf::from(positional[0]),
            })
        }
        "settings-check" => {
            check_arity("settings-check", 1, &positional)?;
            reject_non_record_flags("settings-check", &flags)?;
            Ok(Command::SettingsCheck {
                input: PathBuf::from(positional[0]),
            })
        }
        "settings-show" => {
            check_arity("settings-show", 1, &positional)?;
            reject_non_record_flags("settings-show", &flags)?;
            Ok(Command::SettingsShow {
                input: PathBuf::from(positional[0]),
            })
        }
        "settings-gui" => {
            check_arity("settings-gui", 2, &positional)?;
            reject_non_record_flags("settings-gui", &flags)?;
            Ok(Command::SettingsGui {
                input: PathBuf::from(positional[0]),
                output: PathBuf::from(positional[1]),
            })
        }
        "settings-set" => {
            reject_non_record_flags("settings-set", &flags)?;
            if positional.len() < 3 {
                return Err(UsageError::WrongArity {
                    command: "settings-set",
                    expected: 3,
                    actual: positional.len(),
                });
            }
            Ok(Command::SettingsSet {
                input: PathBuf::from(positional[0]),
                output: PathBuf::from(positional[1]),
                edits: positional[2..]
                    .iter()
                    .map(|value| (*value).clone())
                    .collect(),
            })
        }
        "settings-patch" => {
            reject_non_record_flags("settings-patch", &flags)?;
            if positional.len() < 2 {
                return Err(UsageError::WrongArity {
                    command: "settings-patch",
                    expected: 2,
                    actual: positional.len(),
                });
            }
            Ok(Command::SettingsPatch {
                input: PathBuf::from(positional[0]),
                edits: positional[1..]
                    .iter()
                    .map(|value| (*value).clone())
                    .collect(),
            })
        }
        other => Err(UsageError::UnknownCommand(other.to_string())),
    }
}

/// Parses the M10 `takeover` command (M10_TASK.md §2):
///
/// ```text
/// takeover TRACE [--device DEVICE] --takeover --confirm TAKEOVER
///          --output-qualified --profile m10-linear-v1
///          --max-duration-seconds N
/// ```
///
/// `TRACE` is mandatory. `DEVICE` is optional: when omitted, takeover
/// discovers the unique touchpad candidate at runtime. When multiple
/// candidates exist, runtime refuses and asks for `--device DEVICE`. The
/// legacy positional `takeover DEVICE TRACE ...` spelling remains accepted.
fn parse_takeover(args: &[String]) -> Result<Command, UsageError> {
    let mut device_flag: Option<PathBuf> = None;
    let mut device_count = 0usize;
    let mut takeover_count = 0usize;
    let mut confirm_count = 0usize;
    let mut output_qualified_count = 0usize;
    let mut profile_count = 0usize;
    let mut duration_count = 0usize;
    let mut feel_config_count = 0usize;
    let mut settings_count = 0usize;
    let mut watch_settings_count = 0usize;
    let mut profile: Option<String> = None;
    let mut duration: Option<u32> = None;
    let mut feel_config: Option<PathBuf> = None;
    let mut settings: Option<PathBuf> = None;
    let mut positionals: Vec<PathBuf> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--takeover" => takeover_count += 1,
            "--output-qualified" => output_qualified_count += 1,
            "--device" => {
                device_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::DeviceNeedsValue)?;
                if value.starts_with('-') {
                    return Err(UsageError::DeviceNeedsValue);
                }
                device_flag = Some(PathBuf::from(value));
                i += 1;
            }
            "--confirm" => {
                confirm_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::ConfirmNeedsValue)?;
                if value != TAKEOVER_CONFIRM_TEXT {
                    return Err(UsageError::ConfirmTextMismatch {
                        found: value.clone(),
                    });
                }
                i += 1;
            }
            "--profile" => {
                profile_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::ProfileRequired)?;
                if !ACCEPTED_TAKEOVER_PROFILES.contains(&value.as_str()) {
                    return Err(UsageError::UnknownProfile {
                        found: value.clone(),
                    });
                }
                profile = Some(value.clone());
                i += 1;
            }
            "--max-duration-seconds" => {
                duration_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::DurationNeedsValue)?;
                duration = Some(parse_duration_seconds(value)?);
                i += 1;
            }
            "--feel-config" => {
                feel_config_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::FeelConfigRequired)?;
                feel_config = Some(PathBuf::from(value));
                i += 1;
            }
            "--settings" => {
                settings_count += 1;
                let value = args.get(i + 1).ok_or(UsageError::SettingsRequired)?;
                settings = Some(PathBuf::from(value));
                i += 1;
            }
            "--watch-settings" => watch_settings_count += 1,
            "--grab" | "--emit" => {
                return Err(UsageError::UnknownFlag(arg.clone()));
            }
            _ if arg.starts_with('-') => {
                return Err(UsageError::UnknownFlag(arg.clone()));
            }
            _ => {
                positionals.push(PathBuf::from(arg));
                if positionals.len() > 2 {
                    return Err(UsageError::WrongArity {
                        command: "takeover",
                        expected: 1,
                        actual: positionals.len(),
                    });
                }
            }
        }
        i += 1;
    }

    // Mandatory opt-ins, independently validated (M10_TASK.md §2). Every one
    // of the five mandatory flags is also duplicate-rejected — including
    // `--output-qualified`, `--profile`, and `--max-duration-seconds`,
    // regardless of whether the repeated values agree (M10 review R4).
    if takeover_count == 0 {
        return Err(UsageError::TakeoverFlagRequired);
    }
    if takeover_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag { flag: "--takeover" });
    }
    if confirm_count == 0 {
        return Err(UsageError::ConfirmRequired);
    }
    if confirm_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag { flag: "--confirm" });
    }
    if output_qualified_count == 0 {
        return Err(UsageError::OutputQualifiedRequired);
    }
    if output_qualified_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag {
            flag: "--output-qualified",
        });
    }
    if device_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag { flag: "--device" });
    }
    if profile_count == 0 {
        return Err(UsageError::ProfileRequired);
    }
    if profile_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag { flag: "--profile" });
    }
    if duration_count == 0 {
        return Err(UsageError::DurationRequired);
    }
    if duration_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag {
            flag: "--max-duration-seconds",
        });
    }
    if feel_config_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag {
            flag: "--feel-config",
        });
    }
    if settings_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag { flag: "--settings" });
    }
    if watch_settings_count > 1 {
        return Err(UsageError::DuplicateTakeoverFlag {
            flag: "--watch-settings",
        });
    }
    let profile = profile.ok_or(UsageError::ProfileRequired)?;
    if profile == touchpad_core::m17::M17_TUNABLE_V1_NAME {
        if feel_config.is_none() {
            return Err(UsageError::FeelConfigRequired);
        }
        if settings.is_some() {
            return Err(UsageError::SettingsOnlyM18M19 {
                profile: profile.clone(),
            });
        }
        if watch_settings_count > 0 {
            return Err(UsageError::WatchSettingsOnlyM19 {
                profile: profile.clone(),
            });
        }
    } else if profile == touchpad_core::m18::M18_REMAP_V1_NAME {
        if settings.is_none() {
            return Err(UsageError::SettingsRequired);
        }
        if feel_config.is_some() {
            return Err(UsageError::FeelConfigOnlyM17 {
                profile: profile.clone(),
            });
        }
        if watch_settings_count > 0 {
            return Err(UsageError::WatchSettingsOnlyM19 {
                profile: profile.clone(),
            });
        }
    } else if profile == touchpad_core::m19::M19_LIVE_V1_NAME {
        if settings.is_none() {
            return Err(UsageError::SettingsRequired);
        }
        if feel_config.is_some() {
            return Err(UsageError::FeelConfigOnlyM17 {
                profile: profile.clone(),
            });
        }
        if watch_settings_count == 0 {
            return Err(UsageError::WatchSettingsRequired);
        }
    } else if feel_config.is_some() {
        return Err(UsageError::FeelConfigOnlyM17 {
            profile: profile.clone(),
        });
    } else if settings.is_some() {
        return Err(UsageError::SettingsOnlyM18M19 {
            profile: profile.clone(),
        });
    } else if watch_settings_count > 0 {
        return Err(UsageError::WatchSettingsOnlyM19 {
            profile: profile.clone(),
        });
    }
    let max_duration_seconds = duration.ok_or(UsageError::DurationRequired)?;
    let (legacy_device, trace) = match positionals.as_slice() {
        [trace] => (None, trace.clone()),
        [legacy_device, trace] => (Some(legacy_device.clone()), trace.clone()),
        _ => {
            return Err(UsageError::WrongArity {
                command: "takeover",
                expected: 1,
                actual: positionals.len(),
            });
        }
    };
    if device_flag.is_some() && legacy_device.is_some() {
        return Err(UsageError::DeviceSpecifiedTwice);
    }
    let device = device_flag.or(legacy_device);

    Ok(Command::Takeover {
        device,
        trace,
        max_duration_seconds,
        profile,
        feel_config,
        settings,
        watch_settings: watch_settings_count == 1,
    })
}

/// Parses and range-checks the maximum duration: an integer in
/// `1..=300` seconds. Overflow, malformed, zero, and out-of-range values are
/// rejected (no unlimited form).
fn parse_duration_seconds(value: &str) -> Result<u32, UsageError> {
    let parsed: u32 = value.parse().map_err(|_| UsageError::DurationInvalid {
        found: value.to_string(),
    })?;
    if !(MIN_TAKEOVER_SECONDS..=MAX_TAKEOVER_SECONDS).contains(&parsed) {
        return Err(UsageError::DurationInvalid {
            found: value.to_string(),
        });
    }
    Ok(parsed)
}

/// Rejects every flag for the non-`record` commands (M5 review R5):
/// `--grab` must never be silently accepted by `devices`/`inspect`/`replay`,
/// and any unknown flag is a usage error. M10: takeover-only flags
/// (`--takeover`, `--confirm`, `--output-qualified`, `--profile`,
/// `--max-duration-seconds`) are rejected with a specific diagnostic on
/// every other command before any side effect (M10_TASK.md §2).
fn reject_non_record_flags(command: &'static str, flags: &[&String]) -> Result<(), UsageError> {
    if let Some(flag) = flags.first() {
        if flag.as_str() == "--grab" {
            return Err(UsageError::GrabNotAllowed { command });
        }
        if TAKEOVER_ONLY_FLAGS.contains(&flag.as_str()) {
            return Err(UsageError::TakeoverFlagNotAllowed {
                command,
                flag: flag.to_string(),
            });
        }
        return Err(UsageError::UnknownFlag(flag.to_string()));
    }
    Ok(())
}

fn check_arity(
    command: &'static str,
    expected: usize,
    positional: &[&String],
) -> Result<(), UsageError> {
    if positional.len() == expected {
        Ok(())
    } else {
        Err(UsageError::WrongArity {
            command,
            expected,
            actual: positional.len(),
        })
    }
}

/// The help text. `--grab` carries the mandatory exclusivity/risk warning
/// (M5 acceptance: `--grab` defaults off and the help warns explicitly); the
/// M10 `takeover` section carries the mandatory live-takeover warnings
/// (M10_TASK.md §2).
pub const HELP_TEXT: &str = "\
touchpadctl — Touchpad Runtime Phase 1 command-line tool

USAGE:
  touchpadctl devices
  touchpadctl inspect DEVICE
  touchpadctl record DEVICE OUTPUT [--grab]
  touchpadctl replay INPUT
  touchpadctl output-probe [--emit]
  touchpadctl config-check FILE
  touchpadctl service-preflight FILE
  touchpadctl feel-default OUTPUT
  touchpadctl feel-check FILE
  touchpadctl feel-show FILE
  touchpadctl feel-set INPUT OUTPUT KEY=VALUE [KEY=VALUE ...]
  touchpadctl feel-gui INPUT OUTPUT.html
  touchpadctl settings-default OUTPUT
  touchpadctl settings-macos OUTPUT
  touchpadctl settings-check FILE
  touchpadctl settings-show FILE
  touchpadctl settings-set INPUT OUTPUT KEY=VALUE [KEY=VALUE ...]
  touchpadctl settings-patch FILE KEY=VALUE [KEY=VALUE ...]
  touchpadctl settings-gui INPUT OUTPUT.html
  touchpadctl takeover TRACE [--device DEVICE] --takeover --confirm TAKEOVER \
--output-qualified --profile m10-linear-v1 --max-duration-seconds N

COMMANDS:
  devices              Enumerate /dev/input/event* nodes and explain how each
                       one was judged (candidate / rejected / inaccessible).
  inspect DEVICE       Probe one device node and show its identity,
                       capabilities, axes, slot count, and the candidate
                       verdict with the reasons.
  record DEVICE OUTPUT
                       Record raw evdev events from DEVICE into a versioned
                       JSON Lines trace at OUTPUT. The raw events are written
                       to the trace BEFORE they are decoded, so the trace
                       survives decoder bugs. Stop with Ctrl-C (SIGINT) or
                       SIGTERM: the trace is flushed and the device released
                       cleanly.
  replay INPUT         Replay a raw trace offline through the exact same
                       Type-B decoder used for live input, printing one JSON
                       ContactFrame per line on stdout and a summary on
                       stderr. Purely offline: no /dev/input access.
  output-probe         Probe the KDE Wayland output backend (XDG
                       RemoteDesktop portal + libei) and report the
                       environment, the capabilities that would be
                       negotiated, and the exact steps --emit would run.
                       NON-EMITTING by default: it never moves the pointer,
                       clicks, or scrolls, and never touches /dev/input.
  output-probe --emit  EXPLICIT OPT-IN: run a short, FIXED, BOUNDED test
                       pattern on the real desktop (3 relative pointer
                       moves of 10/50/200 px, a primary click, a
                       pixel-precise smooth scroll, a secondary click),
                       preceded by a visible warning and a 3-second
                       countdown (Ctrl-C to cancel). The backend is
                       EXPERIMENTAL/UNQUALIFIED until a reviewer measures a
                       real --emit run.
  config-check FILE    Strictly parse/validate an M16 runtime JSON file and
                       explicitly migrate v1 in memory. No device/output or
                       service side effect.
  service-preflight FILE
                       Print the M16 foreground-only lifecycle/capability
                       preflight. The reported lifecycle remains Stopped; no
                       service is started or installed.
  feel-default OUTPUT  Write the M16-equivalent M17 FeelConfig v1 defaults.
  feel-check FILE      Strictly validate one M17 FeelConfig.
  feel-show FILE       Print one validated FeelConfig as normalized JSON.
  feel-set INPUT OUTPUT KEY=VALUE [KEY=VALUE ...]
                       Apply validated feel edits and write a new JSON file.
  feel-gui INPUT OUTPUT.html
                       Generate a SELF-CONTAINED OFFLINE HTML tuning panel.
                       The page has no network/device/live-apply path; export
                       JSON and validate/apply it explicitly through CLI.
  settings-default OUTPUT
                       Write M18 UserSettings v1 with M17-equivalent feel and
                       legacy-compatible gesture mappings.
  settings-macos OUTPUT
                       Write the documented macOS-inspired gesture preset.
                       This is a mapping preset, not a macOS-equivalence claim.
  settings-check FILE Strictly validate one unified M18/M19 settings file.
  settings-show FILE  Print one validated settings file as normalized JSON.
  settings-set INPUT OUTPUT KEY=VALUE [KEY=VALUE ...]
                       Apply feel.* and gesture.* edits transactionally and
                       write a new settings file.
  settings-patch FILE KEY=VALUE [KEY=VALUE ...]
                       Apply validated edits IN PLACE. Intended for a running
                       M19 --watch-settings session so changes can be felt
                       without restarting the bounded takeover.
  settings-gui INPUT OUTPUT.html
                       Generate the self-contained combined feel + gesture
                       settings editor. The generated page remains offline;
                       export/save settings.json for M18/M19.
  takeover TRACE [--device DEVICE]
                       M10 BOUNDED LIVE TAKEOVER: exclusively grab the
                       selected physical touchpad (EVIOCGRAB), decode its raw
                       events through the Type-B decoder, resolve them
                       through the approved M7-M9 interaction arbiter with
                       the explicit versioned policy profile (--profile),
                       emit ONLY the resolved semantic pointer/button/scroll
                       events through a PREPARED portal+libei streaming
                       session to the current KDE Wayland desktop, and record
                       the raw input to TRACE — all for at most
                       --max-duration-seconds (1..=300). Foreground-only, no
                       daemon/autostart/service. EXPERIMENTAL: the backend
                       stays experimental/unqualified and M10 stays
                       live-unqualified until the user completes the 10/60/300
                       second acceptance sequence in doc/old/acceptance/M10_ACCEPTANCE.md.
                       WARNING: this GRABS the physical touchpad EXCLUSIVELY,
                       EMITS REAL DESKTOP INPUT (pointer motion, clicks,
                       scroll), and OPENS A PORTAL AUTHORIZATION PROMPT.
                       Keep an EXTERNAL KEYBOARD AND MOUSE connected and keep
                       a SECOND TERMINAL ready to run `kill -TERM <pid>` as an
                       independent escape route. Cleanup after SIGKILL, a
                       kernel crash, or power loss CANNOT be promised (the
                       kernel releases the grab when the fd closes at process
                       exit, but no ordered sequence is guaranteed).
                       --output-qualified is the OPERATOR ATTESTATION that
                       the M6 output calibration was performed — it is NOT
                       measurement evidence and does not qualify anything.
                       If --device is omitted, /dev/input/event* is scanned
                       and the unique touchpad candidate is selected
                       automatically. If multiple candidates qualify,
                       takeover refuses to guess, lists them, and asks you to
                       rerun with --device /dev/input/eventX. The legacy
                       `takeover DEVICE TRACE ...` form remains accepted.

OPTIONS:
  --grab               (record only) Exclusively grab the device with
                       EVIOCGRAB(1) while recording. Rejected for every
                       other command, and may be given at most once.
                       WARNING: while grabbed, this process EXCLUSIVELY owns
                       the touchpad — the desktop and other applications will
                       NOT receive its events, so the pointer, tap, and
                       gesture behavior of the system is unusable until
                       recording stops (or the process exits and the kernel
                       releases the grab). The device must be released
                       cleanly (SIGINT/SIGTERM are handled; SIGKILL, a kernel
                       crash, or power loss cannot run cleanup). Requires
                       permission to grab the device (usually membership in
                       the `input` group or root).
                       Default: OFF — without --grab the device is read
                       without exclusivity.
  --emit               (output-probe only) Explicit opt-in for real desktop
                       emission of the fixed bounded test pattern; see
                       `output-probe --emit` above. Rejected for every other
                       command, and may be given at most once.
  --takeover           (takeover only, mandatory) The explicit opt-in that
                       this is a live exclusive takeover.
  --confirm TAKEOVER   (takeover only, mandatory) The exact confirmation
                       text. Any other value is rejected.
  --output-qualified   (takeover only, mandatory) The operator attestation
                       that the M6 output calibration was performed
                       (doc/old/acceptance/M6_ACCEPTANCE.md §3). Not measurement evidence.
  --device DEVICE      (takeover only, optional) Explicitly choose one
                       /dev/input/event* node. Normally omit this and let a
                       unique touchpad be discovered automatically. Use it
                       when discovery reports multiple candidates. May be
                       given at most once.
  --profile NAME       (takeover only, mandatory) The named versioned policy
                       profile; `m10-linear-v1` (M10 baseline),
                       `m11-fidelity-v1` (EXPERIMENTAL one-finger fidelity),
                       `m12-scroll-v1` (EXPERIMENTAL scroll fidelity +
                       software momentum), or `m13-robust-v1` (EXPERIMENTAL
                       contact robustness), or `m14-gestures-v1` (EXPERIMENTAL
                       continuous gestures), `m15-kde-v1` (EXPERIMENTAL
                       three-finger drag + desktop actions), `m16-production-v1`
                       (configuration-complete), or `m17-tunable-v1` (explicit FeelConfig tuning;
                       requires --feel-config FILE), `m18-remap-v1` (unified
                       feel + configurable gesture actions; requires --settings
                       FILE), or `m19-live-v1` (M18 settings with safe hot
                       reload and real KDE Plasma KGlobalAccel actions;
                       requires --settings FILE --watch-settings). Real M19
                       supports workspace next/previous, Overview, Present
                       Windows, Show Desktop and Application Launcher;
                       unsupported action/native-continuous routes fail
                       capability validation before grab or on reload.
                       No experimental profile is a macOS-equivalence claim or
                       live-qualified by default.
  --feel-config FILE   (takeover only, M17 conditional) REQUIRED exactly for
                       `--profile m17-tunable-v1`; rejected for every other
                       profile. The
                       strict FeelConfig is loaded and validated before any
                       output/device/recorder/grab side effect.
  --settings FILE      (takeover only, M18/M19 conditional) REQUIRED for
                       `m18-remap-v1` and `m19-live-v1`; rejected for M10-M17.
                       The strict UserSettings file is validated before any
                       output/device/recorder/grab side effect.
  --watch-settings     (takeover only, M19 conditional) REQUIRED exactly for
                       `m19-live-v1`. The same settings file is checked on the
                       existing bounded loop cadence. Invalid/partial saves
                       keep the last-good settings; valid changes apply only
                       at a neutral interaction boundary (normally after all
                       fingers/buttons are released).
  --max-duration-seconds N
                       (takeover only, mandatory) The maximum takeover
                       duration as an integer in 1..=300 seconds. No zero,
                       overflow, missing, repeated, or unlimited form is
                       accepted.
  The five M10 takeover safety flags are mandatory and are AT MOST ONCE each:
  any repeat of --takeover, --confirm, --output-qualified, --profile,
  or --max-duration-seconds is a usage error. The M17-only --feel-config is
  also at most once; --settings is at most once for M18/M19; and the M19-only
  --watch-settings opt-in is at most once.
  -h, --help           Show this help.

EXIT CODES:
  0  success; takeover: the session ended (deadline reached, or
     SIGINT/SIGTERM during the loop) with ALL required cleanup succeeding —
     the stderr status line states the exact stop reason
  1  usage / argument error
  2  input directory or device node not found (no /dev/input);
     output-probe: no D-Bus session bus or no RemoteDesktop portal;
     takeover: no session bus / no portal, or the device node is missing
  3  permission denied reading the input directory or device node;
     output-probe: authorization cancelled or refused by the user/portal;
     takeover: authorization cancelled or refused by the user/portal
  4  no touchpad candidate (or the inspected device is not a candidate);
     output-probe: libei library missing, protocol version too old, or a
     required capability missing;
     takeover: libei missing / protocol too old / a required capability
     missing (refused before the recorder or grab)
  5  trace file error (missing, corrupt, schema mismatch, time regression);
     output-probe: transport disconnected or the session timed out;
     takeover: output transport disconnected or timed out during preparation
  6  device stream error (EOF/unplug, torn read, decoder failure) or a
     device-release failure (ungrab/close failed during cleanup);
     output-probe: a send failed (partial send failure);
     takeover: a device stream error, a semantic-output fault (the arbiter
     or the output sink rejected an event), or a device-release failure
  7  recorder error (trace output could not be written or finalized);
     output-probe: releasing held button/key/scroll state failed;
     takeover: recorder output/finalize failure or an output-release failure
  8  stopped by SIGINT/SIGTERM (controlled stop; trace flushed, device
     released — only when the finalization actually succeeded; a failed
     finalization returns 6 or 7 with the full cleanup diagnostic);
     output-probe: aborted by the user before/during emission;
     takeover: aborted by the user before the takeover began (countdown
     cancel / signal during the countdown) — nothing was grabbed, the
     prepared output session was released, the recorder finalized, the
     device closed
  9  unexpected/internal error
";

/// Prints the help text to `out`.
pub fn print_help(out: &mut dyn std::io::Write) -> std::io::Result<()> {
    out.write_all(HELP_TEXT.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn help_flag_parses() {
        assert_eq!(parse(&["--help"]).unwrap(), Command::Help);
        assert_eq!(parse(&["-h"]).unwrap(), Command::Help);
        assert_eq!(parse(&["record", "--help"]).unwrap(), Command::Help);
    }

    #[test]
    fn no_command_is_a_usage_error() {
        assert!(matches!(parse(&[]), Err(UsageError::NoCommand)));
    }

    #[test]
    fn devices_parses() {
        assert_eq!(parse(&["devices"]).unwrap(), Command::Devices);
    }

    #[test]
    fn inspect_parses() {
        assert_eq!(
            parse(&["inspect", "/dev/input/event0"]).unwrap(),
            Command::Inspect {
                device: PathBuf::from("/dev/input/event0")
            }
        );
    }

    #[test]
    fn record_parses_without_grab_by_default() {
        assert_eq!(
            parse(&["record", "/dev/input/event0", "trace.jsonl"]).unwrap(),
            Command::Record {
                device: PathBuf::from("/dev/input/event0"),
                output: PathBuf::from("trace.jsonl"),
                grab: false,
            }
        );
    }

    #[test]
    fn record_grab_is_explicit_opt_in() {
        assert_eq!(
            parse(&["record", "/dev/input/event0", "t.jsonl", "--grab"]).unwrap(),
            Command::Record {
                device: PathBuf::from("/dev/input/event0"),
                output: PathBuf::from("t.jsonl"),
                grab: true,
            }
        );
    }

    /// M5 review R5: `--grab` is rejected for every command except `record`
    /// — `devices --grab`, `inspect DEVICE --grab`, and `replay INPUT
    /// --grab` are usage errors (exit 1) and no command runs.
    #[test]
    fn grab_is_rejected_for_every_command_except_record() {
        assert!(matches!(
            parse(&["devices", "--grab"]),
            Err(UsageError::GrabNotAllowed { command: "devices" })
        ));
        assert!(matches!(
            parse(&["inspect", "/dev/input/event0", "--grab"]),
            Err(UsageError::GrabNotAllowed { command: "inspect" })
        ));
        assert!(matches!(
            parse(&["replay", "trace.jsonl", "--grab"]),
            Err(UsageError::GrabNotAllowed { command: "replay" })
        ));
        // The flag position does not matter: it is still rejected.
        assert!(matches!(
            parse(&["replay", "--grab", "trace.jsonl"]),
            Err(UsageError::GrabNotAllowed { command: "replay" })
        ));
    }

    /// M5 review R5: duplicate `--grab` on `record` is rejected (a repeated
    /// flag is a usage error, not a silent success).
    #[test]
    fn duplicate_grab_is_rejected() {
        assert!(matches!(
            parse(&["record", "/dev/input/event0", "t.jsonl", "--grab", "--grab"]),
            Err(UsageError::DuplicateGrab { count: 2 })
        ));
        assert!(matches!(
            parse(&["record", "--grab", "/dev/input/event0", "--grab", "t.jsonl"]),
            Err(UsageError::DuplicateGrab { count: 2 })
        ));
        // A single --grab still parses.
        assert_eq!(
            parse(&["record", "/dev/input/event0", "t.jsonl", "--grab"]).unwrap(),
            Command::Record {
                device: PathBuf::from("/dev/input/event0"),
                output: PathBuf::from("t.jsonl"),
                grab: true,
            }
        );
    }

    #[test]
    fn replay_parses() {
        assert_eq!(
            parse(&["replay", "trace.jsonl"]).unwrap(),
            Command::Replay {
                input: PathBuf::from("trace.jsonl")
            }
        );
    }

    #[test]
    fn unknown_command_or_flag_is_a_usage_error() {
        assert!(matches!(
            parse(&["frobnicate"]),
            Err(UsageError::UnknownCommand(_))
        ));
        assert!(matches!(
            parse(&["devices", "--verbose"]),
            Err(UsageError::UnknownFlag(_))
        ));
    }

    #[test]
    fn wrong_arity_is_a_usage_error() {
        assert!(matches!(
            parse(&["inspect"]),
            Err(UsageError::WrongArity { .. })
        ));
        assert!(matches!(
            parse(&["record", "/dev/input/event0"]),
            Err(UsageError::WrongArity { .. })
        ));
        assert!(matches!(
            parse(&["record", "a", "b", "c"]),
            Err(UsageError::WrongArity { .. })
        ));
    }

    /// M5 acceptance: the help text must warn explicitly that `--grab`
    /// exclusively owns the touchpad and its risks, and that it is off by
    /// default.
    #[test]
    fn help_warns_explicitly_about_grab_risks() {
        let text = HELP_TEXT;
        assert!(text.contains("--grab"), "help documents --grab");
        assert!(text.contains("EXCLUSIVELY"), "exclusivity warning");
        assert!(text.contains("WARNING"), "warning marker");
        assert!(text.contains("Default: OFF"), "grab defaults off");
        assert!(text.contains("SIGKILL"), "non-guaranteed cleanup stated");
        assert!(text.contains("EVIOCGRAB(1)"), "mentions the ioctl");
    }

    /// M6: `output-probe` defaults to the non-emitting dry-run; `--emit` is
    /// an explicit opt-in; `--emit` on any other command is a usage error
    /// and a duplicate `--emit` is rejected.
    #[test]
    fn output_probe_parses_and_emit_is_an_explicit_opt_in() {
        assert_eq!(
            parse(&["output-probe"]).unwrap(),
            Command::OutputProbe { emit: false }
        );
        assert_eq!(
            parse(&["output-probe", "--emit"]).unwrap(),
            Command::OutputProbe { emit: true }
        );
        // --emit before the command word also parses (flag position is
        // free).
        assert_eq!(
            parse(&["output-probe", "--emit"]).unwrap(),
            Command::OutputProbe { emit: true }
        );
        assert!(matches!(
            parse(&["output-probe", "--emit", "--emit"]),
            Err(UsageError::DuplicateEmit { count: 2 })
        ));
        assert!(matches!(
            parse(&["output-probe", "--grab"]),
            Err(UsageError::GrabNotAllowed {
                command: "output-probe"
            })
        ));
        assert!(matches!(
            parse(&["output-probe", "--verbose"]),
            Err(UsageError::UnknownFlag(_))
        ));
        assert!(matches!(
            parse(&["record", "/dev/input/event0", "t.jsonl", "--emit"]),
            Err(UsageError::UnknownFlag(_))
        ));
        // The help documents the non-emitting default and the countdown.
        assert!(HELP_TEXT.contains("NON-EMITTING by default"));
        assert!(HELP_TEXT.contains("3-second"));
    }

    // ------------------------------------------------------------------
    // M10: takeover command parsing (M10_TASK.md §2)
    // ------------------------------------------------------------------

    /// The canonical full takeover command parses.
    #[test]
    fn takeover_parses_with_all_mandatory_opt_ins() {
        let command = parse(&[
            "takeover",
            "/dev/input/event0",
            "trace.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::Takeover {
                device: Some(PathBuf::from("/dev/input/event0")),
                trace: PathBuf::from("trace.jsonl"),
                max_duration_seconds: 60,
                profile: "m10-linear-v1".to_string(),
                feel_config: None,
                settings: None,
                watch_settings: false,
            }
        );
    }

    #[test]
    fn takeover_can_omit_device_for_auto_discovery() {
        let command = parse(&[
            "takeover",
            "trace.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ])
        .unwrap();
        let Command::Takeover { device, trace, .. } = command else {
            panic!("expected takeover");
        };
        assert_eq!(device, None);
        assert_eq!(trace, PathBuf::from("trace.jsonl"));
    }

    #[test]
    fn takeover_accepts_explicit_device_flag() {
        let command = parse(&[
            "takeover",
            "trace.jsonl",
            "--device",
            "/dev/input/event15",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ])
        .unwrap();
        let Command::Takeover { device, trace, .. } = command else {
            panic!("expected takeover");
        };
        assert_eq!(device, Some(PathBuf::from("/dev/input/event15")));
        assert_eq!(trace, PathBuf::from("trace.jsonl"));
    }

    /// Every mandatory opt-in is independently validated: missing any one of
    /// `--takeover`, `--confirm TAKEOVER`, `--output-qualified`,
    /// `--profile`, or `--max-duration-seconds` is a usage error.
    #[test]
    fn takeover_requires_every_mandatory_opt_in() {
        let base = [
            "takeover",
            "/dev/input/event0",
            "trace.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ];
        // Missing --takeover.
        let without: Vec<&str> = base
            .iter()
            .copied()
            .filter(|a| *a != "--takeover")
            .collect();
        assert!(matches!(
            parse(&without),
            Err(UsageError::TakeoverFlagRequired)
        ));
        // Missing --confirm (and its value).
        let without: Vec<&str> = base
            .iter()
            .copied()
            .filter(|a| *a != "--confirm" && *a != "TAKEOVER")
            .collect();
        assert!(matches!(parse(&without), Err(UsageError::ConfirmRequired)));
        // Missing --output-qualified.
        let without: Vec<&str> = base
            .iter()
            .copied()
            .filter(|a| *a != "--output-qualified")
            .collect();
        assert!(matches!(
            parse(&without),
            Err(UsageError::OutputQualifiedRequired)
        ));
        // Missing --profile (and its value).
        let without: Vec<&str> = base
            .iter()
            .copied()
            .filter(|a| *a != "--profile" && *a != "m10-linear-v1")
            .collect();
        assert!(matches!(parse(&without), Err(UsageError::ProfileRequired)));
        // Missing --max-duration-seconds (and its value).
        let without: Vec<&str> = base
            .iter()
            .copied()
            .filter(|a| *a != "--max-duration-seconds" && *a != "60")
            .collect();
        assert!(matches!(parse(&without), Err(UsageError::DurationRequired)));
    }

    /// The confirmation text must be exactly `TAKEOVER`; any other value is
    /// rejected, and `--confirm` without a value is rejected.
    #[test]
    fn takeover_confirm_text_is_exact_and_required() {
        let wrong = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "yes",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            wrong,
            Err(UsageError::ConfirmTextMismatch { found }) if found == "yes"
        ));
        let no_value = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(
            matches!(no_value, Err(UsageError::ConfirmTextMismatch { found }) if found == "--output-qualified")
        );
    }

    /// Duration validation: 0, 301, malformed, negative, and overflowing
    /// values are rejected; 1 and 300 are accepted (boundaries).
    #[test]
    fn takeover_duration_is_validated_with_boundaries() {
        for bad in ["0", "301", "abc", "-5", "1.5", "99999999999999999999"] {
            let result = parse(&[
                "takeover",
                "/dev/input/event0",
                "t.jsonl",
                "--takeover",
                "--confirm",
                "TAKEOVER",
                "--output-qualified",
                "--profile",
                "m10-linear-v1",
                "--max-duration-seconds",
                bad,
            ]);
            assert!(
                matches!(result, Err(UsageError::DurationInvalid { .. })),
                "duration {bad:?} must be rejected"
            );
        }
        for ok in ["1", "300"] {
            let result = parse(&[
                "takeover",
                "/dev/input/event0",
                "t.jsonl",
                "--takeover",
                "--confirm",
                "TAKEOVER",
                "--output-qualified",
                "--profile",
                "m10-linear-v1",
                "--max-duration-seconds",
                ok,
            ]);
            let Command::Takeover {
                max_duration_seconds,
                ..
            } = result.unwrap()
            else {
                panic!("expected takeover");
            };
            assert_eq!(max_duration_seconds.to_string(), ok);
        }
        // Missing value for --max-duration-seconds.
        let no_value = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
        ]);
        assert!(matches!(no_value, Err(UsageError::DurationNeedsValue)));
    }

    /// Repeated opt-in flags are rejected (no silent duplicate acceptance).
    #[test]
    fn takeover_duplicate_flags_are_rejected() {
        let dup = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            dup,
            Err(UsageError::DuplicateTakeoverFlag { flag: "--takeover" })
        ));
        let dup_confirm = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            dup_confirm,
            Err(UsageError::DuplicateTakeoverFlag { flag: "--confirm" })
        ));
    }

    /// M10 review R4: repeats of **all five** mandatory flags are rejected —
    /// including `--output-qualified`, `--profile`, and
    /// `--max-duration-seconds` — regardless of whether the repeated values
    /// agree (a duplicate is a usage error, never a silent overwrite).
    #[test]
    fn takeover_duplicate_flags_of_all_five_are_rejected() {
        // Repeated --output-qualified (a bare flag: any repeat is a
        // duplicate).
        let dup_qualified = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            dup_qualified,
            Err(UsageError::DuplicateTakeoverFlag {
                flag: "--output-qualified"
            })
        ));

        // Repeated --profile with an IDENTICAL value is still rejected.
        let dup_profile_same = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            dup_profile_same,
            Err(UsageError::DuplicateTakeoverFlag { flag: "--profile" })
        ));

        // Repeated --max-duration-seconds with a CONFLICTING value is
        // rejected (the second value never silently overwrites the first).
        let dup_duration_conflict = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
            "--max-duration-seconds",
            "300",
        ]);
        assert!(matches!(
            dup_duration_conflict,
            Err(UsageError::DuplicateTakeoverFlag {
                flag: "--max-duration-seconds"
            })
        ));

        // A repeated --max-duration-seconds with an IDENTICAL value is also
        // rejected.
        let dup_duration_same = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            dup_duration_same,
            Err(UsageError::DuplicateTakeoverFlag {
                flag: "--max-duration-seconds"
            })
        ));
    }

    /// Only `m10-linear-v1` is an accepted profile.
    #[test]
    fn takeover_profile_is_validated() {
        let unknown = parse(&[
            "takeover",
            "/dev/input/event0",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "macos-like",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(
            unknown,
            Err(UsageError::UnknownProfile { found }) if found == "macos-like"
        ));
    }

    /// The accepted profile set is explicit and ordered; no profile is
    /// inferred when `--profile` is absent.
    #[test]
    fn takeover_profile_accepts_exactly_the_documented_set() {
        let base = |profile: &str| {
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
        };
        assert_eq!(
            ACCEPTED_TAKEOVER_PROFILES,
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
        for profile in ACCEPTED_TAKEOVER_PROFILES {
            let mut args = base(profile);
            if *profile == touchpad_core::m17::M17_TUNABLE_V1_NAME {
                args.push("--feel-config".to_string());
                args.push("feel.json".to_string());
            } else if *profile == touchpad_core::m18::M18_REMAP_V1_NAME {
                args.push("--settings".to_string());
                args.push("settings.json".to_string());
            } else if *profile == touchpad_core::m19::M19_LIVE_V1_NAME {
                args.push("--settings".to_string());
                args.push("settings.json".to_string());
                args.push("--watch-settings".to_string());
            }
            let command = parse_args(args).unwrap();
            let Command::Takeover { profile: got, .. } = command else {
                panic!("expected takeover");
            };
            assert_eq!(&got, profile);
        }
        // The unknown-profile error names the accepted set accurately.
        let unknown = parse_args(base("macos-like")).unwrap_err();
        match &unknown {
            UsageError::UnknownProfile { found } => assert_eq!(found, "macos-like"),
            other => panic!("expected UnknownProfile, got {other:?}"),
        }
        let text = unknown.to_string();
        assert!(text.contains("m10-linear-v1"), "{text}");
        assert!(text.contains("m11-fidelity-v1"), "{text}");
        assert!(text.contains("m12-scroll-v1"), "{text}");
        assert!(text.contains("m13-robust-v1"), "{text}");
        assert!(text.contains("m14-gestures-v1"), "{text}");
        assert!(text.contains("m15-kde-v1"), "{text}");
        assert!(text.contains("m16-production-v1"), "{text}");
        assert!(text.contains("m17-tunable-v1"), "{text}");
        assert!(text.contains("m18-remap-v1"), "{text}");
        assert!(text.contains("m19-live-v1"), "{text}");

        assert!(matches!(
            parse_args(base("m17-tunable-v1")),
            Err(UsageError::FeelConfigRequired)
        ));
        let mut wrong = base("m16-production-v1");
        wrong.extend(["--feel-config".to_string(), "feel.json".to_string()]);
        assert!(matches!(
            parse_args(wrong),
            Err(UsageError::FeelConfigOnlyM17 { .. })
        ));
        assert!(matches!(
            parse_args(base("m18-remap-v1")),
            Err(UsageError::SettingsRequired)
        ));
        let mut m18_watch = base("m18-remap-v1");
        m18_watch.extend([
            "--settings".to_string(),
            "settings.json".to_string(),
            "--watch-settings".to_string(),
        ]);
        assert!(matches!(
            parse_args(m18_watch),
            Err(UsageError::WatchSettingsOnlyM19 { .. })
        ));
        let mut m19_no_watch = base("m19-live-v1");
        m19_no_watch.extend(["--settings".to_string(), "settings.json".to_string()]);
        assert!(matches!(
            parse_args(m19_no_watch),
            Err(UsageError::WatchSettingsRequired)
        ));
        // No profile is inferred when --profile is absent.
        let without: Vec<String> = base("m10-linear-v1")
            .into_iter()
            .filter(|a| a != "--profile" && a != "m10-linear-v1")
            .collect();
        assert!(matches!(
            parse_args(without),
            Err(UsageError::ProfileRequired)
        ));
    }

    /// Takeover-only flags are rejected on every other command (usage error
    /// before any side effect).
    #[test]
    fn takeover_flags_are_rejected_elsewhere() {
        for (command, args) in [
            ("devices", vec!["devices", "--takeover"]),
            (
                "inspect",
                vec!["inspect", "/dev/input/event0", "--output-qualified"],
            ),
            (
                "record",
                vec!["record", "/dev/input/event0", "t.jsonl", "--takeover"],
            ),
            ("replay", vec!["replay", "t.jsonl", "--confirm", "TAKEOVER"]),
            (
                "output-probe",
                vec!["output-probe", "--profile", "m10-linear-v1"],
            ),
        ] {
            let result = parse(&args);
            assert!(
                matches!(result, Err(UsageError::TakeoverFlagNotAllowed { command: c, .. }) if c == command),
                "{command}: {result:?}"
            );
        }
    }

    /// Missing device/trace paths are arity errors; `--grab`/`--emit` are not
    /// takeover flags.
    #[test]
    fn takeover_requires_trace_but_device_may_be_discovered() {
        let missing = parse(&[
            "takeover",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ]);
        assert!(matches!(missing, Err(UsageError::WrongArity { .. })));
        let one = parse(&[
            "takeover",
            "t.jsonl",
            "--takeover",
            "--confirm",
            "TAKEOVER",
            "--output-qualified",
            "--profile",
            "m10-linear-v1",
            "--max-duration-seconds",
            "60",
        ])
        .unwrap();
        assert!(matches!(
            one,
            Command::Takeover {
                device: None,
                trace,
                ..
            } if trace.as_path() == std::path::Path::new("t.jsonl")
        ));
        // --grab / --emit are not valid takeover flags.
        assert!(matches!(
            parse(&[
                "takeover",
                "/dev/input/event0",
                "t.jsonl",
                "--takeover",
                "--confirm",
                "TAKEOVER",
                "--output-qualified",
                "--profile",
                "m10-linear-v1",
                "--max-duration-seconds",
                "60",
                "--grab",
            ]),
            Err(UsageError::UnknownFlag(_))
        ));
    }

    /// M10 acceptance: the help text documents the takeover command with the
    /// mandatory warnings (exclusive grab, real desktop input, portal prompt,
    /// experimental, external keyboard/mouse + second-terminal SIGTERM, no
    /// SIGKILL cleanup promise).
    #[test]
    fn help_documents_takeover_with_mandatory_warnings() {
        let text = HELP_TEXT;
        assert!(text.contains("takeover DEVICE TRACE"), "usage line");
        assert!(text.contains("--takeover"), "opt-in flag");
        assert!(text.contains("--confirm TAKEOVER"), "confirmation flag");
        assert!(text.contains("--output-qualified"), "attestation flag");
        assert!(text.contains("m10-linear-v1"), "profile");
        assert!(text.contains("1..=300"), "bounded duration");
        assert!(text.contains("EXCLUSIVELY"), "exclusivity warning");
        assert!(
            text.contains("EMITS REAL DESKTOP INPUT"),
            "emission warning"
        );
        assert!(
            text.contains("PORTAL AUTHORIZATION PROMPT"),
            "portal warning"
        );
        assert!(text.contains("EXPERIMENTAL"), "experimental warning");
        assert!(
            text.contains("EXTERNAL KEYBOARD AND MOUSE"),
            "escape warning"
        );
        assert!(text.contains("kill -TERM"), "second-terminal escape");
        assert!(text.contains("SIGKILL"), "no SIGKILL cleanup promise");
        // M10 review R4: the help documents that EVERY mandatory takeover
        // flag is rejected when repeated.
        assert!(
            text.contains("AT MOST ONCE"),
            "the help documents the at-most-once rule for every takeover flag"
        );
        // The help text breaks lines between the fragments; assert each.
        assert!(text.contains("it is NOT"), "attestation honesty");
        assert!(text.contains("measurement evidence"), "attestation honesty");
    }
}
