# M19 Task — Safe Live Settings Hot Reload

Authority: follows M18 unified user settings and preserves all M10 takeover
safety/cleanup boundaries.

## Goal

Let users edit the same `settings.json` while a bounded takeover is running and
feel changes without restarting the session.

## Contract

- Add `m19-live-v1`, inheriting M18 settings/gesture mapping. M19 applies two
  live-use refinements to the inherited M8 tap policy: one completed tap arms
  the immediately-following one-finger contact for tap-and-drag for **180 ms**,
  matching the local libinput single-finger drag timeout; and sticky
  one-finger tap-drag lock is disabled, so a committed drag emits its matching
  left-button release on the clean `Ended` frame rather than holding left into
  the next interaction. M10-M18 profile behavior is unchanged.
- M19 also installs a three-finger-drag-only pointer-fidelity profile. It
  preserves the ordinary pointer dead-zone, low-speed gain and tracking speed,
  while capping the drag high-speed gain at 1.6. The ordinary one-finger
  pointer fidelity is not reduced.
- M19 refines only the live three-finger motion model after comparison with
  `linux-3-finger-drag`: the centroid still decides when the gesture commits,
  but the commit frame is only a classification/baseline boundary and does
  **not** replay the accumulated classifier displacement. Committed motion is
  then measured from one stable tracking-id reference finger. If that reference
  lifts while original contacts remain, a surviving original finger is chosen
  and re-baselined with zero motion on the switch frame. Synthetic left is
  pressed only when the first real post-commit PointerMove is emitted. M15-M18
  retain their historical centroid/replay behavior.
- In that M19 stable-reference mode, a committed drag keeps ownership through
  clean `3 -> 2 -> 1` staggered lifts and releases only when the original
  contact cluster is empty. Remaining contacts never fall through to pointer
  or scroll while that drag is owned.
- M19's portal/libei output keeps drag ownership edges and their owned motion
  in one EIS logical hardware frame (`ButtonDown + first PointerMove`, or
  `final PointerMove + ButtonUp`). Tap click pulses remain two distinct libei
  frames. This preserves the core semantic event ordering while avoiding a
  compositor-visible split between press/release state and relative motion.
- M19 requires explicit `--settings FILE --watch-settings` in addition to all
  existing bounded takeover opt-ins. No watcher is inferred by earlier profiles.
- Watch the settings file on the existing bounded loop cadence (about 100 ms;
  faster while momentum already requires a faster loop). No background daemon.
- Reload is last-good/fail-open:
  - unchanged file: no work;
  - valid changed file: build a complete new ArbiterConfig atomically;
  - invalid/partial save: report `reload rejected`, retain current config, keep running;
  - a later valid save recovers automatically.
- Never change policy in the middle of ownership. A valid update applies only
  when pointer/two-finger/continuous-gesture/three-finger-drag/momentum/button
  ownership is quiescent. Otherwise exactly one latest pending update is kept
  and applied at the next neutral boundary (normally after lifting fingers).
- Config replacement resets only tunable filter/router state at that neutral
  boundary; cleanup/safety/device/output state is unchanged.
- Add `settings-patch FILE KEY=VALUE...` for convenient in-place edits while a
  watcher is running. It uses the exact M18 validation and writes only a fully
  valid settings document.
- No network listener, remote control, arbitrary shell execution, systemd, or
  implicit autostart is added.

### Real KDE Plasma output extension

- The production `m19-live-v1` backend is a composite output session:
  pointer/button/pixel-scroll remain on the existing RemoteDesktop
  portal+libei path, while discrete `DesktopAction` events use KDE Plasma 6
  KGlobalAccel over the D-Bus session bus.
- The production action transport uses the existing component
  `org.kde.kglobalaccel.Component` interface. It performs a read-only
  `shortcutNames()` preflight and invokes only the closed built-in action set
  through `invokeShortcut(action_id)`; it never runs shell commands.
- The currently supported real KDE actions are exactly: next workspace,
  previous workspace, Overview, Present Windows (`Expose`), Show Desktop and
  Application Launcher.
- Notification Center, page next/previous, Smart Zoom, Lookup and native
  `ContinuousGesture` passthrough have no qualified real transport in M19.
  A production M19 settings file containing any of these routes is rejected
  before device/output/recorder/grab side effects. The same capability check
  is applied to watched reloads; an unsupported edit is rejected and the
  last-good configuration keeps running.
- The macOS-inspired preset contains only the six currently executable KDE
  actions and disables unsupported continuous/action routes. This remains a
  layout inspiration, not a macOS-equivalence claim.
- Implementation/review may introspect KGlobalAccel read-only, but must not
  call `invokeShortcut`, move workspaces, open Overview, or otherwise trigger
  a real desktop action. Real action delivery is user-run acceptance only.

## Exit

Tests cover idle apply, busy queue/latest-wins, invalid reload last-good,
automatic recovery, in-place patch, and no changes to M10 cleanup. Full fmt,
clippy, debug and release workspace gates must pass. M19 remains
live-unqualified until user-run live acceptance.
