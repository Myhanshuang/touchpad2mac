# M18 Task — Configurable Gesture → Desktop Action Mapping

Authority: follows approved M17 tunable-feel milestone. M18 must preserve all
M10 takeover safety/cleanup boundaries and must not make desktop-neutral core
depend on KDE-specific identifiers.

## Goal

Allow users to assign recognized gestures to different built-in desktop
functions in a settings file, comparable to a macOS trackpad gesture settings
panel, while retaining an explicit passthrough mode for applications that want
continuous pinch/rotate/swipe events.

## Contract

- Add strict `GestureMapConfig` v1 in `touchpad-core`.
- Gesture triggers are directional/semantic, including pinch in/out,
  clockwise/counter-clockwise rotate, directional 2/3/4-finger and edge
  swipes, thumb+three pinch/spread, and three-finger tap.
- Every trigger resolves to `passthrough`, `disabled`, or one typed
  `DesktopAction` target.
- A mapped continuous gesture fires exactly one action on Begin, suppresses
  its Update/End stream, and resets on End/Cancel.
- Three-finger tap uses the same mapping table. Default remains Lookup.
- `GestureMapConfig::default()` must reproduce M17 behavior exactly.
- Provide a documented `macos-inspired` preset. The original M18 mapping
  covered page/workspace swipes, overview/app-expose style vertical swipes,
  launcher/show-desktop pinch, notification-center edge swipe and Lookup tap.
  After the M19 real-KDE integration, the generated `settings-macos` preset is
  intentionally narrowed to the real M19 executable subset (workspace,
  overview/present-windows, launcher/show-desktop); unsupported page,
  notification, lookup and native-continuous routes default to disabled.
- Three-finger drag and three-finger swipe share the same contacts. The
  gesture settings therefore expose `three_finger_drag_enabled`: default true
  preserves M15/M17; the macOS-inspired preset sets it false so three-finger
  swipes are reachable. Disabling drag commit must retain three-finger tap.
- Add strict `UserSettings` v1 wrapping M17 `FeelConfig` + M18 gesture map.
- Add `m18-remap-v1`; default UserSettings must preserve all M17 feel and
  ownership decisions except the inert routing layer.
- Add CLI settings operations: default/check/show/set/preset/gui.
- M18 takeover requires explicit `--settings FILE`; earlier profiles reject it.
- M18 does not execute arbitrary shell commands. Desktop action execution stays
  behind the existing typed/injected action transport and honestly reports
  unavailable when no real transport is configured.

## Exit

Tests cover schema validation, directional routing, single-fire suppression,
three-finger tap remapping, default compatibility, macOS-inspired preset,
CLI/GUI editing and takeover routing. Final fmt, clippy, debug workspace tests
and release workspace tests must pass. M18 remains live-unqualified until
user-run acceptance.
