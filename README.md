# touchpad2mac

`touchpad2mac` is an experimental userspace touchpad runtime for Linux, focused on macOS-inspired pointer feel, gesture semantics, and three-finger drag on KDE Plasma Wayland.

The project reads Linux evdev multitouch input, decodes Type-B contact frames, resolves competing interactions in a platform-independent arbiter, and emits desktop input through the XDG RemoteDesktop portal + libei. Discrete KDE actions are routed through Plasma's KGlobalAccel interface.

> **Status:** active development. The current codebase is extensively covered by offline and fake-backed tests, but the live takeover profiles remain **live-unqualified**. Do not treat this project as a drop-in replacement for libinput or as a claim of macOS-equivalent behavior.

## Highlights

- Automatic discovery of compatible `/dev/input/event*` touchpads.
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

The current real-output path targets:

- Linux
- KDE Plasma 6 on Wayland
- XDG Desktop Portal RemoteDesktop support
- runtime-available `libei.so.1`
- a readable Linux evdev touchpad device

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

Workspace layout:

| Path | Purpose |
| --- | --- |
| `crates/touchpad-core` | Platform-independent interaction policy, gesture recognition, settings, and output contracts |
| `crates/touchpad-trace` | Versioned JSONL trace format and replay boundary |
| `crates/touchpad-linux` | Linux evdev enumeration, Type-B decoding, recording, grabbing, and runtime boundary |
| `crates/touchpad-desktop` | Portal/libei output and KDE desktop-action integration |
| `apps/touchpadctl` | Command-line frontend and bounded live takeover orchestration |

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
- The project is not a complete replacement for libinput and does not yet carry libinput's hardware quirk database or platform maturity.
- Keyboard discovery / full disable-while-typing integration is incomplete.
- Some semantic gesture targets remain unsupported by the real KDE transport.
- Native continuous gesture passthrough is not currently a production output path.
- Hardware pressure, haptics, and Force Click are outside the current production scope.

## Documentation

- [Run guide](doc/RUN_GUIDE.md) — practical build, setup, takeover, and tuning workflow.
- [User manual](doc/USER_MANUAL.md) — settings, gestures, tuning behavior, and recovery guidance.
- [Historical documents](doc/old/) — milestone tasks, design records, acceptance procedures, and review reports retained for traceability.
- [Third-party notes](THIRD_PARTY.md) — external components and related integration notes.

Historical documents describe the state of the project at specific milestones and may intentionally contain superseded behavior or paths. Use this README and the two current documents under `doc/` as the primary reference.

## Project status

The current development line is centered on `m19-live-v1`: configurable macOS-inspired gesture behavior, stable three-finger drag, safe settings hot reload, automatic touchpad discovery, and KDE Plasma integration. Earlier `m10-*` through `m18-*` profiles remain in the codebase mainly as compatibility and regression boundaries.

## License

The Rust workspace is declared as dual-licensed under **MIT OR Apache-2.0**. Dedicated license text files have not yet been added to the repository.
