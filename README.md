# touchpad2mac

`touchpad2mac` is an experimental cross-platform touchpad runtime. Linux is the qualified development path for full evdev takeover; Windows now has a native platform boundary for Precision Touchpad discovery, capability probing, and semantic mouse/button/wheel output, with full physical-device takeover intentionally gated on a filter driver.

The project reads Linux evdev multitouch input, decodes Type-B contact frames, resolves competing interactions in a platform-independent arbiter, and emits desktop input through the XDG RemoteDesktop portal + libei. Discrete KDE actions are routed through Plasma's KGlobalAccel interface.

> **Status:** active development. The current codebase is extensively covered by offline and fake-backed tests, but the live takeover profiles remain **live-unqualified**. Do not treat this project as a drop-in replacement for libinput or as a claim of macOS-equivalent behavior.

## Highlights

- Automatic discovery of compatible `/dev/input/event*` touchpads.
- Windows Precision Touchpad discovery through the Raw Input HID device list.
- Windows compatibility output through `SendInput`, plus runtime probing for the newer native synthetic Precision Touchpad API.
- Libinput-inspired disable-while-typing (DWT) using read-only monitoring of paired internal keyboards; no keyboard grab or key logging.
- One-finger pointer motion with configurable dead zone, tracking speed, and gain curve.
- Tap, double tap, tap-and-drag, physical click, and secondary click handling.
- Two-finger pixel scrolling with direction filtering and axis locking.
- Three-finger drag with stable reference-finger tracking and explicit button ownership.
- Pinch, rotate, multi-finger swipe, edge-swipe, and thumb-plus-three gesture recognition.
- Configurable gesture-to-desktop-action mapping.
- KDE Plasma actions including workspace switching, Overview, Present Windows, Show Desktop, and Application Launcher.
- Strict versioned user settings with validation before live takeover.
- Safe settings hot reload at neutral interaction boundaries.
- Versioned JSONL traces for recording, replay, regression testing, and debugging.
- Bounded live takeover sessions with explicit opt-in and ordered cleanup.

## Platform

The current full-takeover path targets:

- Linux
- KDE Plasma 6 on Wayland
- XDG Desktop Portal RemoteDesktop support
- runtime-available `libei.so.1`
- a readable Linux evdev touchpad device

Windows support currently provides a safe **user-mode overlay/probe layer**:

- Precision Touchpads are identified by the HID Digitizers/Touch Pad top-level collection (`usage page 0x0D`, `usage 0x05`).
- `touchpadctl windows-probe` enumerates visible PTP devices and reports Windows API availability without emitting input.
- `touchpadctl windows-capture OUTPUT.jsonl SECONDS` performs a bounded, read-only Raw Input capture on Windows 10/11. It registers only the PTP top-level collection, never registers a keyboard, and never injects input; the resulting JSONL contains raw touchpad HID reports and may encode touch positions.
- `touchpad-windows` contains a tested semantic output sink for relative pointer motion, buttons, and Win32 wheel data using `SendInput` on Windows.
- Pure-Rust PTP hybrid-report assembly and three-finger overlay ownership are covered by tests while the hardware-specific HID descriptor decoder is being wired to real capture data.
- New Windows 11 synthetic touchpad exports are detected dynamically, so older Windows builds fail closed rather than failing process startup.
- **Full takeover is not claimed.** Windows Raw Input can observe HID data but does not provide a user-mode equivalent of `EVIOCGRAB` for Precision Touchpads. A signed HID/mouse-class filter driver is required before the physical touchpad can be suppressed without duplicate native input.

The workspace declares Rust **1.87** as its MSRV.

## Build

```bash
git clone git@github.com:Myhanshuang/touchpad2mac.git
cd touchpad2mac
cargo build --release --locked
```

The CLI binary is:

```bash
target/release/touchpadctl
```

For normal Linux installation, use the packaged user-service layout instead
of running a bounded takeover command manually:

```bash
./packaging/install.sh
touchpadctl doctor ~/.config/touchpad2mac/settings.json
systemctl --user enable --now touchpad2mac.service
```

Pass `--enable` to `packaging/install.sh` to enable/start the service during
installation. The installer keeps runtime files user-scoped; only the udev
`uaccess` rule requires administrator privileges. It never makes input devices
world-readable/world-writable.

On Windows, the first bring-up command is:

```powershell
target\release\touchpadctl.exe windows-probe
target\release\touchpadctl.exe windows-capture ptp-capture.jsonl 30
```

During the 30-second capture, exercise one-finger motion, two-finger scrolling,
three-finger tap/drag, and a four-finger gesture. Native Windows touchpad
behavior remains active throughout this diagnostic capture.

For development, the standard quality gates are:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Quick start

### 1. Inspect available touchpads

```bash
target/release/touchpadctl devices
```

You can inspect one device explicitly:

```bash
target/release/touchpadctl inspect /dev/input/event15
```

### 2. Create a settings file

Start from the default settings:

```bash
target/release/touchpadctl settings-default settings.json
target/release/touchpadctl settings-check settings.json
```

Or create the macOS-inspired gesture preset:

```bash
target/release/touchpadctl settings-macos settings.json
target/release/touchpadctl settings-check settings.json
```

The preset describes interaction layout only; it is not a macOS-equivalence claim.

### 3. Check desktop output support

```bash
target/release/touchpadctl output-probe
```

`output-probe` is non-emitting unless the explicit `--emit` option is supplied.

### 4. Run a bounded M19 takeover session

Keep an external mouse/keyboard and a second terminal available before using live takeover.

```bash
target/release/touchpadctl takeover trace-m19.jsonl \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m19-live-v1 \
  --settings settings.json \
  --watch-settings \
  --max-duration-seconds 300
```

`takeover` scans `/dev/input/event*` automatically:

- if exactly one compatible touchpad is found, it is selected automatically;
- if multiple candidates are found, the command stops before portal setup, recording, or `EVIOCGRAB` and prints the required `--device` alternatives;
- an explicit device can always be selected with `--device /dev/input/eventX`.

Example with an explicit device:

```bash
target/release/touchpadctl takeover trace-m19.jsonl \
  --device /dev/input/event15 \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m19-live-v1 \
  --settings settings.json \
  --watch-settings \
  --max-duration-seconds 300
```

Use **Ctrl-C** or **SIGTERM** for normal shutdown so the runtime can execute its ordered release, ungrab, and session cleanup path.

### Production service

Installed service mode runs:

```bash
touchpadctl service-run ~/.config/touchpad2mac/settings.json
```

`service-run` reuses the same exclusive touchpad ownership, arbiter,
portal/libei output, DWT, settings watcher and ordered cleanup as `takeover`,
but it has no artificial 300-second development deadline/countdown and does
not continuously record an unbounded raw touch trace. `takeover` remains the
explicit reproduction/qualification path.

Before enabling the service on a new machine, run:

```bash
touchpadctl doctor ~/.config/touchpad2mac/settings.json
```

For support or new hardware qualification:

```bash
touchpadctl diagnostics diagnostics.json
touchpadctl qualify qualification.json
```

The static diagnostics bundle contains device/session metadata and applied
quirks only. It intentionally contains no keyboard key codes and no touch
trace. Touch traces remain explicit, separate artifacts created only by the
record/takeover tools.

## Configuration

Current user configuration is stored in a strict `UserSettings v1` JSON document. It combines feel parameters and gesture routing:

```text
settings.json
├── version
├── feel
│   ├── pointer
│   ├── scroll
│   ├── gesture
│   └── drag
├── gestures
└── dwt
    ├── enabled
    ├── short_timeout_ms
    └── long_timeout_ms
```

Settings can be changed from the CLI:

```bash
target/release/touchpadctl settings-patch settings.json \
  feel.pointer.tracking_speed=1.20
```

Or edited through the generated offline HTML editor:

```bash
target/release/touchpadctl settings-gui settings.json settings.html
```

The HTML editor is self-contained. It does not connect to the input runtime, open devices, or perform live apply by itself.

DWT is enabled by default. The runtime automatically pairs internal typing keyboards, opens their evdev nodes **read-only**, selects `CLOCK_MONOTONIC`, and immediately reduces qualifying key presses to anonymous timestamps. Raw key codes are never written to the touch trace or forwarded into `touchpad-core`. The default timing follows libinput's short/continued typing model: **200 ms** after an isolated key press and **500 ms** while typing continues. Standalone modifiers do not arm DWT, and keyboard activity does not interrupt an already committed pointer, scroll, gesture, or drag interaction.

With `m19-live-v1 --watch-settings`, a valid settings update is applied immediately when the interaction state is neutral. If an interaction is active, only the newest valid generation is queued and applied after returning to a neutral boundary. Invalid or partially-written JSON is rejected while the last-known-good configuration remains active.

## Architecture

```text
Linux evdev
    │
    ▼
Type-B multitouch decoder
    │
    ├──────────────► versioned JSONL trace / replay
    │
    ▼
normalized ContactFrame
    │
    ▼
interaction arbiter
    │
    ├── pointer / click / scroll
    ├── tap / tap-drag
    ├── three-finger drag
    └── continuous gestures
    │
    ▼
typed output decisions
    │
    ├── XDG RemoteDesktop + libei ──► pointer / buttons / scroll
    └── KDE KGlobalAccel ───────────► desktop actions
```

Windows currently uses a separate platform edge rather than pretending the
Linux evdev model applies unchanged:

```text
Windows Raw Input / HID device enumeration
    │
    ├── Precision Touchpad identity probe (0x0D / 0x05)
    │
    ├── bounded raw HID capture + hybrid-report/contact assembler
    │
    └── HID descriptor decoder / filter-driver takeover boundary

touchpad-core semantic OutputEvent
    │
    ├── SendInput compatibility output (implemented)
    └── native synthetic PTP API (runtime capability probe implemented)
```

Workspace layout:

| Path | Purpose |
| --- | --- |
| `crates/touchpad-core` | Platform-independent interaction policy, gesture recognition, settings, and output contracts |
| `crates/touchpad-trace` | Versioned JSONL trace format and replay boundary |
| `crates/touchpad-linux` | Linux evdev enumeration, Type-B decoding, recording, grabbing, and runtime boundary |
| `crates/touchpad-windows` | Windows Precision Touchpad discovery/capability boundary and tested Win32 semantic output |
| `crates/touchpad-desktop` | Portal/libei output and KDE desktop-action integration |
| `crates/touchpad-testkit` | Optional Linux `/dev/uinput` software-in-the-loop fixtures that traverse the real kernel evdev path |
| `apps/touchpadctl` | Command-line frontend and bounded live takeover orchestration |

Hardware-specific corrections live in the strict, versioned
`quirks/builtin.json` database rather than product-name branches spread across
the runtime. Unknown hardware uses the generic profile; proposed quirk entries
are schema-validated in tests.

## CLI overview

The main commands are:

```text
devices
inspect
record
replay
output-probe
config-check
service-preflight
doctor
diagnostics
qualify
service-run
feel-default / feel-check / feel-show / feel-set / feel-gui
settings-default / settings-macos / settings-check / settings-show
settings-set / settings-patch / settings-gui
takeover
```

Run the built-in help for the authoritative argument list:

```bash
target/release/touchpadctl --help
```

## Safety model

Live takeover is intentionally difficult to start accidentally. It requires all of the following:

- an explicit `takeover` command;
- `--takeover`;
- exact confirmation text `--confirm TAKEOVER`;
- `--output-qualified` operator attestation;
- an explicit policy profile;
- a bounded `--max-duration-seconds` value in `1..=300`.

The runtime performs validation before device/output side effects where possible and uses ordered cleanup for normal termination and handled failures. `SIGKILL`, kernel failure, power loss, compositor failure, and hardware/driver bugs cannot be made fully recoverable by userspace code.

## Current limitations

- Live profiles are still **live-unqualified** and require user-run hardware/session acceptance.
- The production output path currently targets KDE Plasma Wayland; X11 and generic desktop support are not production paths.
- The project is not a complete replacement for libinput and its hardware quirk database is still very small compared with libinput's platform maturity.
- DWT discovery/timing is implemented, but broad laptop keyboard/touchpad pairing still needs community hardware qualification.
- Some semantic gesture targets remain unsupported by the real KDE transport.
- Native continuous gesture passthrough is not currently a production output path.
- Hardware pressure, haptics, and Force Click are outside the current production scope.

## Documentation

- [Run guide](doc/RUN_GUIDE.md) — practical build, setup, takeover, and tuning workflow.
- [User manual](doc/USER_MANUAL.md) — settings, gestures, tuning behavior, and recovery guidance.
- [Architecture](doc/ARCHITECTURE.md) — typed platform boundaries, recognizer ownership, output lifecycle, and production/runtime split.
- [Hardware support](doc/HARDWARE.md) — diagnostics, qualification tiers, and evidence-driven quirk contributions.
- [Historical documents](doc/old/) — milestone tasks, design records, acceptance procedures, and review reports retained for traceability.
- [Third-party notes](THIRD_PARTY.md) — external components and related integration notes.
- [Packaging](packaging/README.md) — installation layout, systemd service, and udev access rule.
- [Contributing](CONTRIBUTING.md) — architecture invariants, tests, hardware evidence, and PR expectations.
- [Security](SECURITY.md) — private vulnerability reporting and input/privacy boundaries.

Historical documents describe the state of the project at specific milestones and may intentionally contain superseded behavior or paths. Use this README and the two current documents under `doc/` as the primary reference.

## Project status

The current production policy is derived from `m19-live-v1`: configurable macOS-inspired gesture behavior, stable three-finger drag, safe settings hot reload, automatic touchpad discovery, DWT, KDE Plasma integration, systemd service packaging, data-driven quirks and system-level uinput tests. Earlier `m10-*` through `m18-*` profiles remain in the codebase mainly as compatibility and regression boundaries; end users should use `service-run` rather than choosing milestone profiles directly.

## License

The project is dual-licensed under **MIT OR Apache-2.0**. See
`LICENSE-MIT` and `LICENSE-APACHE`.
