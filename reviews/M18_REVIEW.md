# M18 Review — Configurable Gesture Mapping

Date: 2026-08-23

## Verdict

**APPROVED — code-complete / review-approved; live-unqualified.**

M18 adds a strict, desktop-neutral user settings layer for mapping recognized
gestures onto typed built-in `DesktopAction` semantics while preserving
passthrough/disabled modes. It does not add arbitrary shell-command execution
or silently claim a real KDE action transport that the current stack has not
qualified.

## Reviewed implementation

- `touchpad-core::gesture_bindings`
  - strict `GestureMapConfig v1`;
  - directional pinch/rotate/page/3f/4f/edge/thumb+3 triggers;
  - `GestureTarget::{Passthrough, Disabled, ...typed actions...}`;
  - mapped continuous gestures fire once at Begin and suppress Update/End;
  - three-finger tap uses the same mapping table.
- `touchpad-core::settings`
  - strict `UserSettings v1 = FeelConfig + GestureMapConfig`;
  - transactional `feel.*` and `gesture.*` key editing.
- `m18-remap-v1`
  - inherits M17 feel policy;
  - adds only gesture routing / three-finger drag-vs-swipe policy.
- `touchpadctl`
  - `settings-default`, `settings-macos`, `settings-check`, `settings-show`,
    `settings-set`, `settings-patch`, `settings-gui`;
  - M18 bounded takeover requires explicit `--settings FILE` and rejects the
    M17-only `--feel-config` / M19-only `--watch-settings` paths.
- self-contained settings GUI has no network/device/live-apply path.

## Reviewer finding and repair

### R1 — three-finger swipe mappings were initially unreachable

The first implementation correctly mapped M14 three-finger swipe events, but
M15 three-finger drag intentionally committed at a smaller movement threshold
than M14 multi-swipe. With the M17 ownership policy unchanged, three-finger
drag won first, so a configured three-finger swipe action in the macOS-inspired
preset could never actually fire.

Repair:

- `GestureMapConfig` now exposes `three_finger_drag_enabled`;
- default is `true`, preserving M15/M17 behavior;
- the macOS-inspired preset sets it to `false`;
- `ThreeFingerDragConfig::with_drag_enabled(false)` disables only **drag
  commit**, while retaining the existing three-finger tap candidate;
- M18 applies that setting when constructing the inherited M15 stage;
- the GUI/manual exposes the conflict explicitly.

Regression evidence now proves both:

1. macOS preset three-finger swipe-up reaches `DesktopAction::OpenOverview`
   without emitting a synthetic left-button drag;
2. with drag commit disabled, three-finger tap still emits its configured
   `Lookup` action.

R1 is closed.

## Security / capability review

- New M18/M19 settings code contains no process-spawn or shell-command path.
  The only `std::process` reference in the touched takeover path prints the
  current PID for the existing `kill -TERM <pid>` escape instruction.
- M18 maps only to the closed typed `DesktopAction` enum.
- Current M15 KDE action transport remains an injected/qualification boundary;
  the existing M6 portal/libei sink does not thereby become a qualified KDE
  action executor. This is stated in the acceptance guide and user manual.

## Final gates

Run after R1 was repaired:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
```

The final workspace run includes the dedicated M18 integration tests and all
earlier M1–M17 regressions.

## Qualification boundary

M18 is **not live-qualified**. `docs/M18_ACCEPTANCE.md` is written but no real
M18 takeover or real KDE desktop-action acceptance was executed during this
milestone. A correct typed action in core is not evidence that the real KDE
transport has been qualified.

### Later M19 compatibility note — 2026-08-23

M19 subsequently added the production KDE Plasma KGlobalAccel executor. To
make `settings-macos` safe to use directly with real M19, the preset was
narrowed to the six executable KDE semantics and now disables unsupported
page/notification/lookup/native-continuous routes. This does not change M18's
typed routing model or its live qualification. The reviewer invariant from R1
still holds: disabling three-finger drag commit leaves the tap candidate
available, and an explicit three-finger-tap mapping is still tested.
