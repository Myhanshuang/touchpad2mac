# M19 Review — Safe Live Settings Hot Reload

Date: 2026-08-23

## Verdict

**APPROVED — code-complete / review-approved; live-unqualified.**

M19 adds a foreground, bounded-loop settings watcher to the existing M10
takeover architecture. It does not add a daemon, network listener, autostart,
or an alternate cleanup path.

## Reviewed implementation

- `m19-live-v1` uses the same complete M18 `UserSettings` / gesture policy,
  with libinput-style deferred tap commit: a qualifying tap release exposes
  `ButtonDown(Left)` while holding the matching up for the 180 ms follow-up
  window; a follow-up contact reuses that press for drag instead of generating
  another down. Inherited one-finger sticky tap-and-drag lock is disabled so a
  committed drag releases left on the clean contact `Ended` frame. M10-M18
  profile behavior remains unchanged.
- CLI requires, simultaneously and explicitly:
  - every existing M10 takeover opt-in;
  - `--profile m19-live-v1`;
  - `--settings FILE`;
  - `--watch-settings`.
- settings are initially loaded/validated before output/device/recorder/grab
  side effects.
- `SettingsWatcher` hashes file bytes on the existing bounded loop cadence:
  - unchanged -> no work;
  - valid changed file -> complete replacement config;
  - malformed/invalid changed file -> `reload rejected`, current last-good
    config remains active;
  - later valid save -> normal recovery.
- only the latest valid-but-busy configuration is retained.
- `settings-patch` provides convenient validated in-place edits using the same
  strict schema as offline settings editing.

## Neutral-boundary safety review

`Arbiter::try_replace_config` refuses replacement while pointer/two-finger
ownership, scroll/momentum, continuous gesture, three-finger drag/lock, or
physical/synthetic button ownership is active. A pending valid update is
therefore applied only at a neutral interaction boundary, normally after all
fingers/buttons are released and momentum has ended.

At a successful neutral replacement, only tunable filter/router residue is
reset (pointer fidelity, scroll fidelity, gesture router, three-finger drag
state, pixel remainders). The device/output/recorder/cleanup lifecycle is not
replaced. Faulted/stopped sink paths refuse reconfiguration.

Automated evidence covers:

- active interaction rejects replacement;
- neutral boundary accepts it;
- invalid changed file preserves last-good and later valid save recovers;
- multiple pending updates are latest-wins;
- all previous M10 fault/cleanup regression tests still pass.

## Final gates

Run after the M18 reviewer repair and all M19 changes:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
```

The final touchpadctl unit suite reports 115 passing tests, plus the public CLI
integration suite and the full core/linux/desktop/trace debug and release
regressions.

## Qualification boundary

M19 is **not live-qualified**. No real device grab, portal/libei desktop input,
or real settings hot-reload session was run while implementing/reviewing this
milestone. The user-run procedure is `docs/M19_ACCEPTANCE.md`.

## Re-review 1 — Real KDE Plasma integration (2026-08-23)

### Trigger for re-review

The user's first real M19 run reached a healthy grabbed/output session and then
failed when a mapped three-finger gesture produced
`DesktopAction(PreviousWorkspace)`. The existing M6 portal/libei sink correctly
rejected that semantic event because M18/M19 previously stopped at the typed
`DesktopAction` boundary. Cleanup succeeded (`output release`, recorder,
ungrab, close), so the failure exposed a missing desktop adapter rather than a
takeover lifecycle defect.

### Repair reviewed

- Added a real KDE Plasma 6 `KGlobalAccelTransport` over the existing zbus
  dependency. It uses the session-bus
  `org.kde.kglobalaccel.Component` interface and a closed mapping:
  - next workspace -> KWin `Switch to Next Desktop`;
  - previous workspace -> KWin `Switch to Previous Desktop`;
  - Overview -> KWin `Overview`;
  - Present Windows -> KWin `Expose`;
  - Show Desktop -> KWin `Show Desktop`;
  - Application Launcher -> PlasmaShell `activate application launcher`.
- `KdeActionStreamingOutput` composes the action channel with the existing
  streaming session. `DesktopAction` goes to KGlobalAccel; pointer, buttons
  and pixel scroll remain unchanged on portal+libei. KDE actions own no held
  state, so `release_all` remains the existing streaming cleanup path.
- The production binary now leaves real streaming selection to takeover.
  Only real `m19-live-v1` chooses the KDE composite; injected fake factories
  and M10-M18 behavior remain unchanged.
- Real M19 validates the complete gesture map before any device/output/grab
  side effect. Notification Center, Page Next/Previous, Smart Zoom, Lookup and
  native `ContinuousGesture` passthrough are explicitly unsupported rather
  than allowed to fault after grab.
- KGlobalAccel `shortcutNames()` preflight is read-only and occurs before
  portal authorization/grab. Hot reload applies the same static capability
  validation: unsupported edits are `reload rejected` and last-good remains
  active.
- The generated `settings-macos` preset was narrowed to the executable KDE
  subset above; unsupported page/notification/lookup/native-continuous routes
  default to `disabled`. This is still only a macOS-inspired layout.

### Real desktop evidence used by review

Read-only introspection of the user's current desktop established:

```text
session: KDE Plasma / Wayland
plasmashell: 6.7.4
kwin: 6.7.4
org.kde.kglobalaccel owner: kwin_wayland

Switch to Next Desktop: PRESENT
Switch to Previous Desktop: PRESENT
Overview: PRESENT
Expose: PRESENT
Show Desktop: PRESENT
activate application launcher: PRESENT
```

The reviewer did **not** call `invokeShortcut`, switch workspaces, open
Overview, show the desktop, start a live takeover, grab the touchpad, or emit
portal/libei desktop input. Real action delivery remains user-run acceptance.

### Regression evidence

New automated coverage proves:

- a discrete `DesktopAction` is routed to the KDE action adapter and never
  appears in the libei fake event stream;
- pointer events continue through the inner streaming output;
- KDE preflight failure occurs before inner portal preparation;
- cleanup still delegates exactly once to the existing streaming session;
- unsupported real-KDE hot-reload settings are rejected and a later valid
  save recovers;
- the KDE-safe macOS preset contains exactly the six supported semantic
  actions;
- all existing M10-M19 tests continue to pass.

Static review found no new Cargo dependency, no new unsafe block, no arbitrary
shell/process execution path, and no credential match in non-generated project
text. `touchpad-desktop` KDE/streaming modules remain `#![forbid(unsafe_code)]`.

Final gates were rerun after all real-KDE changes and documentation updates:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
```

### Re-review 1 verdict

**APPROVED FOR USER LIVE ACCEPTANCE — M19 real KDE integration is
code-complete/review-approved; M19 remains live-unqualified.**

The next evidence must come from the bounded user-run procedure in
`docs/M19_ACCEPTANCE.md`. Passing that procedure qualifies only the tested
machine/session/action set; it is not a macOS-equivalence claim and does not
qualify unsupported action targets.

## Re-review 2 — Live timestamp-domain repair (2026-08-23)

### Trigger

The user's next real M19 run processed 1,720 frames and then stopped with:

```text
timestamp regression: frame timestamp Monotonic(24220015246)
precedes the previous frame timestamp Monotonic(150412504000)
```

Ordered cleanup again succeeded. Inspection showed this was not a real evdev
timestamp regression: live input frames use kernel `CLOCK_MONOTONIC` since
boot (~150 s in the observed run), while the takeover deadline/momentum driver
used `Instant::elapsed()` since process start (~24 s). `Arbiter::tick()`
correctly rejected the mixed epochs.

### Repair

- Added `Arbiter::last_input_timestamp()` and `last_input_sequence()` as
  read-only anchors for the runtime boundary.
- Added `InputDomainTickClock` in `touchpadctl`:
  - observes the process scheduling timestamp and latest accepted input-frame
    marker together;
  - maps only the elapsed process duration onto the kernel/trace input epoch;
  - re-anchors only when frame sequence changes, so an evdev read containing
    no complete new frame cannot erase elapsed momentum time;
  - never weakens the core timestamp-regression check.
- M12 momentum ticks now receive the mapped input-domain timestamp instead of
  the raw process-relative deadline clock.

Regression tests explicitly cover the observed `150 s input / 24 s process`
shape, re-anchoring on a newer frame, and refusing to re-anchor when the frame
sequence did not advance. Existing M12 momentum tests remain green.

The user-facing complete settings were also revised so three-finger drag is
actually enabled without competing with three-finger workspace swipes:

- three-finger drag: enabled;
- drag lock: disabled for predictable lift-to-release behavior;
- three-finger swipes: disabled;
- four-finger left/right: next/previous workspace;
- four-finger up/down: Overview/Present Windows;
- thumb+three pinch/spread: launcher/show desktop;
- unsupported real-KDE targets remain disabled.

Canonical files: `settings-full.json` and the current `settings.json`.

### Final evidence after repair

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

### Re-review 2 verdict

**APPROVED FOR ANOTHER BOUNDED USER LIVE RUN.** The timestamp-domain bug is
fixed in code and regression-covered. M19 remains live-unqualified until the
repaired binary completes the documented live acceptance without this fault.

## Re-review 3 — First hand-feel calibration + directional Overview (2026-08-23)

### User live feedback

After the timestamp repair, the user reported four concrete feel/semantic
issues from the real KDE/Wayland session:

1. one-finger pointer motion felt under-sensitive and needed stronger
   acceleration;
2. three-finger drag inherited the same under-sensitive travel and also felt
   late to engage;
3. four-finger up/down should mean enter Overview / leave Overview rather than
   Overview / Present Windows;
4. vertical two-finger scrolling lost the intended vertical feel too easily
   when the fingers moved slightly diagonally.

### Repair / calibration

- `settings-full.json` and current `settings.json` now use:
  - pointer dead-zone `0.09 -> 0.06 mm`;
  - pointer tracking speed `1.00 -> 1.15`;
  - pointer min gain `1.00 -> 1.05`;
  - pointer max gain `2.00 -> 2.60`;
  - three-finger drag commit threshold `1.00 -> 0.80 mm`;
  - scroll axis-lock engage ratio `2.50 -> 1.60`;
  - scroll axis-lock release ratio `1.50 -> 1.20`.
- Three-finger drag still reuses the same pointer fidelity stage; the lower
  commit threshold changes ownership latency, while pointer gain/speed changes
  travel after commit.
- Inspection of the two-finger path confirmed there is no additional
  same-direction/coherence gate: scroll commits from centroid displacement.
  The diagonal-tolerance complaint therefore maps directly to the M12 axis
  lock thresholds.
- Added typed `DesktopAction::CloseOverview` and `GestureTarget::CloseOverview`
  (`close-overview`). The real KDE transport maps it to the same registered
  KWin `Overview` shortcut but first reads `/Effects` `activeEffects`:
  - `open-overview` invokes the toggle only when `overview` is inactive;
  - `close-overview` invokes it only when `overview` is active.
  This prevents a downward gesture from opening Overview when it is already
  closed.
- The complete settings map four-finger up to `open-overview` and four-finger
  down to `close-overview`; four-finger left/right remain workspace navigation.

### Evidence

Read-only live introspection confirmed KWin exposes `/Effects`, its
`activeEffects` property, and the `overview` effect identifier. No Overview
toggle was invoked during review.

Focused tests cover directional toggle decisions and a four-finger-down
`CloseOverview` routing integration. Final gates after all changes:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

### Re-review 3 verdict

**APPROVED FOR THE NEXT BOUNDED HAND-FEEL RUN.** These are user-derived
calibration changes, not a universal hardware default. M19 remains
live-unqualified until the updated pointer/drag/scroll/Overview behavior is
confirmed in the user's next bounded session.

## Re-review 4 — Tap-drag click-through repair (2026-08-23)

### User live symptom and trace evidence

The user reported an intermittent KDE symptom: tapping a foreground window
could sometimes result in a desktop icon behind/under that window being
dragged. This was treated as an input ownership leak rather than assumed to be
a KWin stacking bug.

Read-only inspection of the already-recorded `tuning-r3.jsonl` showed that the
real CIRQ1080 device can produce very short re-touches. One concrete contact
(tracking id 832) existed from `5290.899980 s` to `5290.948525 s`, about
48.5 ms. Its raw position moved only from `(1902,817)` to `(1900,812)`; at the
device's 24 units/mm resolution that is approximately 0.22 mm, far below a
real pointer/drag commitment.

The previous M8 follow-up contract nevertheless emitted synthetic
`ButtonDown(Left)` immediately on any valid follow-up `Began` inside the
tap-and-drag gap. A short re-touch could therefore expose a held-left wire for
tens of milliseconds without any drag motion, creating exactly the dangerous
focus/stacking race reported by the user.

### Repair

- Added `TapDragPhase::TapDragCandidate`.
- A follow-up `Began` now creates only this pending candidate and emits no
  button edge.
- Only a real pointer commitment calls `prepare_pointer_commit`, which emits
  the synthetic down before the first committed move and transitions to
  `TapDragContact`.
- A clean, uncommitted second tap emits its complete down/up click pulse only
  at release, preserving desktop double-click semantics without an
  intermediate held-left state.
- Pending tracking-id replacement, cancellation, missing coordinates, or
  competing ownership emits no synthetic down.
- Existing accepted-prefix semantics remain meaningful: if the committed
  `[down, move]` decision accepts the down and rejects the move, cleanup still
  owes and retries the up exactly once.

Dedicated regression
`follow_up_tracking_bounce_cannot_turn_a_single_tap_into_drag_through`
reproduces first tap -> follow-up -> tracking-id replacement -> later ordinary
pointer commit and asserts that the complete button stream remains only the
first tap's down/up, with no synthetic held-left leakage. Core tap, double-tap,
drag-lock and M11 fidelity integration tests were updated to the new pending
phase and pass.

No real device grab, desktop action, pointer emission, or KWin action was run
by the repair/review; the trace analysis was read-only. Live confirmation is
still required on the next bounded M19 run.

### Final evidence after Re-review 4

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

The dedicated drag-through regression is part of the 264-test core unit
suite. The full command completed with exit code 0 after the pending
tap-and-drag contract, M11/public M8 integration expectations, documentation,
and takeover comments were synchronized.

### Re-review 4 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** The eager follow-up held-left leak
is removed in code and regression-covered. Because the original symptom was
intermittent and desktop-visible, only repeated live tapping on the user's KDE
session can close the live qualification of this specific defect.

## Re-review 5 — Immediate tap-drag release on finger lift (2026-08-24)

### Trigger

The next user hand-feel run reported that after double-tap/tap-and-drag, the
dragged pointer/object remained logically held for too long after finger lift,
interfering with the following action.

### Root cause and repair

- M19 still inherited the M10/M8 tap policy with
  `drag_lock_enabled=true`. This is independent of M17's
  `feel.drag.drag_lock`, which configures only three-finger drag.
- Therefore a committed one-finger tap-drag could enter
  `LockedWithoutContact` on lift and intentionally retain synthetic left
  ownership into a later interaction.
- M19 now applies a profile-local latency refinement with
  `TapConfig::without_drag_lock()`. Tap-to-click, tap-and-drag enablement,
  maximum tap duration, maximum tap movement and follow-up gap are unchanged.
- M10-M18 profiles retain the historical sticky-lock policy; only
  `m19-live-v1` changes.
- A clean `Ended` frame after a committed M19 tap-drag now emits the matching
  `ButtonUp(Left)` on that same frame and leaves both synthetic and aggregate
  left ownership false.

Dedicated M19 tests prove both the profile separation and the release timing:

```text
m19_disables_one_finger_sticky_drag_lock_only                 PASS
m19_tap_drag_releases_left_on_the_clean_ended_frame           PASS
```

Final gates after the change:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

### Re-review 5 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** M19 no longer carries one-finger
tap-drag held-left ownership past a clean finger lift. Live confirmation should
verify that the next click/gesture can begin immediately after releasing the
drag without an extra unlock tap or observable software hold interval.

## Re-review 6 — Double-tap drag trigger + staggered three-finger release (2026-08-24)

### Trigger

The next user live run reported two interaction mismatches:

1. one completed tap followed by pointer motion still entered tap-and-drag,
   while the intended M19 gesture is two completed taps followed by a later
   sliding contact;
2. dragging application/desktop icons with three fingers could visually drift
   from the pointer and land slightly offset when the fingers were lifted.

### One-finger repair — superseded by deferred release

The earlier `double_tap_before_drag` workaround has been removed. M19 now
follows the libinput commit model instead of adding an extra tap gesture.

- a qualifying first tap emits `ButtonDown(Left)` only and enters the 180 ms
  follow-up window;
- if the window expires with no follow-up contact, the policy emits the owed
  `ButtonUp(Left)` and the click completes;
- if one new finger arrives inside the window, it inherits that already-held
  left press; pointer commitment produces movement without a second down;
- a clean second tap resolves the prior click (`Up`) and immediately starts a
  new deferred press (`Down`), preserving multi-tap behavior;
- physical-button competition, tracking replacement, multi-finger ownership
  and discontinuity resolve/cancel the pending press deterministically;
- M19 still disables one-finger sticky drag lock, so a committed tap-drag's
  clean `Ended` frame releases left immediately.

### Three-finger drag repair

Read-only analysis of the user's latest `tuning-r5.jsonl` found nine
three-finger-or-more contact clusters. Four of the nine did not go directly
`3 -> 0`; they had a one-frame staggered release tail such as `3 -> 2 -> 0` or
`3 -> 1 -> 0`, lasting about 6.1-6.7 ms.

The previous `ThreeFingerDragPhase::Dragging` implementation emitted
`EndDrag` as soon as the live set stopped being exactly the original three
contacts. That released left on the first finger's lift. The remaining one/two
contacts were then free to fall through to lower pointer/scroll policy on the
next frame, so the object could already be dropped while the cursor still
moved, producing the reported visual drift/drop offset.

The repaired policy keeps a committed drag owned through a clean staggered
lift tail:

- when the live set is a non-empty subset of the original three tracking ids,
  the drag remains in `Dragging`;
- no movement is emitted while finger count differs, because 3-finger and
  2/1-finger centroids are not geometrically comparable;
- `blocks_contact_policy=true` throughout the tail, so remaining contacts
  cannot become pointer or scroll ownership;
- with drag lock disabled, the unique `EndDrag`/left-up occurs only after the
  contact cluster is empty;
- a replacement/new tracking id is not treated as a clean lift and still ends
  drag fail-closed.

Dedicated recognizer/output regressions:

```text
committed_drag_waits_for_all_original_fingers_to_lift         PASS
staggered_lift_keeps_three_finger_drag_owned_until_cluster_is_empty PASS
```

### Final evidence after Re-review 6

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpad-core unit suite                                        268 PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

The full chained gate command completed with exit code 0. No live desktop
action or real takeover was performed during this repair; `tuning-r5.jsonl`
was inspected read-only.

### Re-review 6 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** M19 now distinguishes ordinary
single-tap-followed-by-motion from the requested double-tap-armed drag, and a
committed three-finger drag no longer releases ownership on the first finger
of a staggered lift. Live confirmation should focus on exact drag trigger
semantics and icon/cursor coincidence through release.

## Re-review 7 — Restore double-tap drag arm + reduce in-motion drag trail (2026-08-24)

### Trigger

The next bounded user run reported that the newly required
`two complete taps -> later sliding contact` drag was no longer practically
reachable, and that three-finger icon dragging still showed a directional
offset **during** motion rather than only at the staggered release tail.

### Double-tap drag-arm repair

The state machine itself retained the two-complete-tap chain, but M19 still
inherited M10's original 350 ms `max_tap_drag_gap`. That timing was designed
for the earlier immediate follow-up gesture and is too narrow after requiring
the second tap to complete before a third contact begins.

M19 now refines only that follow-up timing to **600 ms** via
`TapConfig::with_max_tap_drag_gap`:

- one completed tap followed by motion still clears the chain and remains
  ordinary pointer movement;
- a second completed tap arms the next contact for 600 ms;
- a dedicated regression places the third contact 500 ms after the second
  tap's release, outside M10's 350 ms window but inside M19's new window, and
  proves `ButtonDown(Left) -> PointerMove` still commits;
- M10-M18 retain their original 350 ms value.

### In-motion three-finger drag analysis and repair

Read-only analysis of `tuning-r6.jsonl` found nine stable three-finger
clusters. Their SYN_REPORT cadence is roughly 165 Hz. Stable per-frame
three-finger centroid steps reached about **1.43 mm**. The current display is
`3072x1920@120 Hz` (logical `2048x1280`, scale 1.5). With the current complete
pointer feel (`tracking_speed=1.25`, `max_gain=2.9`, base 10 logical px/mm), a
single fast three-finger input frame could therefore produce roughly 52
logical pixels of cursor motion. This is enough for multiple high-gain cursor
updates to become visibly ahead of the compositor-rendered drag item and
matches the user's report that the offset points along the previous movement
direction.

M19 now separates three-finger-drag fidelity from ordinary pointer fidelity:

- a new optional `ArbiterConfig::three_finger_drag_fidelity_config` is used
  only by committed `BeginDrag`/`Move` motion;
- M15-M18 leave that field `None` and therefore preserve their old behavior;
- M19 clones the ordinary pointer dead-zone, velocity curve, base scale,
  low-speed gain and tracking speed, but caps the drag-only high-speed
  `max_gain` at **1.6**;
- the ordinary pointer remains at the user's configured `max_gain` (2.9 in
  the current complete settings), so the earlier single-finger sensitivity
  tuning is not rolled back;
- for the observed 1.43 mm extreme frame, the mathematical high-speed ceiling
  falls from about 52 logical px to about 29 logical px.

This is intentionally a drag-path refinement rather than a global pointer
slowdown. The staggered-lift ownership repair from Re-review 6 remains intact.

### Final evidence after Re-review 7

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpad-core unit suite                                        268 PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

The full chained gate command completed with exit code 0. No real desktop
action or live takeover was performed during this repair. `tuning-r6.jsonl`
and the current KScreen output were inspected read-only.

### Re-review 7 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** The double-tap drag is now armed
long enough for the requested three-contact interaction, and three-finger drag
no longer shares the ordinary pointer's very high 2.9 gain ceiling. Live
confirmation is still required to determine whether the compositor-visible
icon/cursor trail is fully eliminated or merely reduced; if a residual
one-refresh-frame trail remains, the next layer to qualify is output-frame /
presentation synchronization rather than further ownership changes.

## Re-review 8 — Restore tap-and-drag semantics + libei drag-frame alignment (2026-08-24)

### Trigger and corrected interpretation

The next bounded user run corrected the tap gesture requirement and exposed a
more specific three-finger failure pattern:

1. tap-and-drag is the conventional **one tap followed immediately by a second
   contact that moves**, not the Re-review 6/7 two-tap + third-contact gesture;
   the original usability defect was excessive stickiness/timeout, not the
   trigger shape;
2. only the first three-finger drag after process startup began at the pointer.
   Every later drag could start with the icon displaced opposite the previous
   drag vector, with occasional unstable drop position.

Read-only inspection of the local libinput device reports `Tap-and-drag:
enabled` and `Tap drag lock: disabled`. The current libinput tap implementation
uses a 180 ms single-finger drag follow-up timeout, matching its 180 ms tap
timeout. M19 now uses that timing rather than the prior 350/600 ms experiments.

### R7 trace finding: recognizer state was not stale

Read-only analysis of `tuning-r7.jsonl` found six distinct three-finger drag
clusters. Every cluster used a completely new tracking-id set and was separated
from the next by a fully empty contact interval of roughly 0.6–2.0 seconds.
Therefore the systematic previous-vector offset could not be explained by a
stale `ThreeFingerDragState::{anchor,last,ids}` surviving between interactions.
The recognizer was already returning to `Idle` and re-anchoring correctly.

### Tap-and-drag repair

`m19-live-v1` now explicitly applies:

```text
max_tap_drag_gap       = 180 ms
drag_lock_enabled      = false
```

The resulting contract is:

- qualifying tap -> deferred `ButtonDown(Left)` at release;
- no follow-up by 180 ms -> matching `ButtonUp(Left)` completes the click;
- next one-finger contact beginning at or before 180 ms reuses the held press
  and may commit `PointerMove` tap-and-drag without another down;
- a contact beginning strictly after 180 ms is ordinary pointer input;
- committed drag releases left on its clean `Ended` frame; no sticky lock.

Dedicated M19 tests cover the profile values, a successful short follow-up,
and strict expiry at 181 ms.

### Root output-frame defect and repair

The real portal/libei adapter previously called `ei_device_frame` from every
individual `OutputSink::submit`. A single core `ContactFrame` decision such as

```text
ButtonDown(Left), PointerMove(dx, dy)
```

therefore became two EIS logical frames:

```text
ButtonDown -> ei_device_frame
PointerMove -> ei_device_frame
```

even though libei's frame boundary represents one logical hardware event. This
split let KWin observe drag ownership and its corresponding relative motion at
different protocol-frame boundaries. It is a stronger match for the reported
cross-drag icon-origin displacement than stale centroid state, which `r7`
directly ruled out.

The output contract now adds `OutputSink::submit_frame` plus structured
`OutputFrameError { failed_index, accepted_prefix, primary }`. Default/test
sinks retain historical event-by-event behavior. `ArbiterSink::frame/tick`
submit one semantic decision through the frame API while preserving the exact
known accepted prefix for reconciliation and cleanup.

`PortalOutputSink` overrides the frame API only for safe drag-edge pairs:

```text
ButtonDown + PointerMove  -> button, motion, one ei_device_frame
PointerMove + ButtonUp    -> motion, button-up, one ei_device_frame
```

A tap pulse remains intentionally separate:

```text
ButtonDown -> frame -> ButtonUp -> frame
```

so the same button is never requested twice inside one libei frame. The M19
KDE composite delegates non-DesktopAction runs through this frame API, so the
real production path retains the grouping rather than losing it at the
KGlobalAccel wrapper.

Wire-level fake-transport regressions prove the exact call sequences, including
two consecutive drag starts separated by a clean release; the second start has
no previous-frame wire residue.

### Three-finger release boundary

The R7 trace also contains common `3 -> 2 -> 1 -> 0` tails. M19/core now aligns
the committed drag release boundary with current libinput three-finger drag
semantics:

- `3 -> 2`: keep drag ownership; emit no motion from the incomparable
  two-finger centroid; block scroll;
- `3 -> 1`: emit `EndDrag` / unique left up on that frame;
- the surviving finger may establish a fresh pointer interaction only on a
  later frame.

This replaces the Re-review 6 behavior that held the drag until the whole
cluster reached zero and reduces delayed/unstable drop behavior when one finger
remains on the pad longer than the others.

### Final evidence after Re-review 8

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpad-core unit suite                                        268 PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

Focused new regressions also pass:

```text
m19_refines_one_finger_tap_drag_to_libinput_aligned_short_follow_up PASS
m19_single_tap_then_follow_up_motion_drags_and_releases_on_lift     PASS
m19_tap_drag_arm_expires_strictly_after_180_ms                       PASS
submit_frame_keeps_drag_press_and_first_motion_in_one_libei_frame    PASS
submit_frame_keeps_final_motion_and_drag_release_in_one_libei_frame  PASS
submit_frame_keeps_tap_down_up_as_two_libei_frames                   PASS
consecutive_drag_starts_each_begin_from_a_fresh_libei_frame          PASS
staggered_lift_keeps_two_finger_tail_owned_and_releases_at_one_finger PASS
```

The full chained gate command completed with exit code 0. This repair used
only read-only analysis of `tuning-r7.jsonl`, local libinput/KScreen state and
offline/fake-backed tests; no real takeover or desktop action was triggered by
the implementation pass.

### Re-review 8 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** The previously incorrect
double-tap/600 ms interpretation has been removed, and the real libei path now
preserves the drag ownership/motion hardware-frame boundary. Live confirmation
must specifically repeat several alternating-direction three-finger drags to
verify that the second-and-later icon origin no longer carries the previous
drag vector, and verify the shortened 180 ms tap-drag arm does not interfere
with subsequent ordinary pointer actions.

## Re-review 9 — Stable reference-finger drag model after linux-3-finger-drag comparison (2026-08-24)

### Trigger

The R8 bounded live run confirmed that one-finger tap-and-drag was restored,
but the three-finger icon-origin displacement remained. It also appeared on
the first drag, disproving explanations that require stale state from an older
drag. The user requested a direct comparison with the open-source
`linux-3-finger-drag` implementation.

### Upstream comparison

Inspection of `linux-3-finger-drag/src/runtime/gesture.rs` found two material
differences from the M15 motion model:

1. drag classification/commit does not replay the motion accumulated while
   deciding that the gesture is a drag; commit establishes a motion baseline;
2. committed motion follows one stable reference finger rather than the
   three-finger centroid. If that reference disappears while original fingers
   remain, a surviving finger becomes the new reference and is re-baselined
   without outputting a jump. The synthetic mouse press is tied to actual drag
   motion rather than the classifier threshold crossing itself.

These are motion-model differences, not KDE action, settings-watch or libei
capability differences.

### R8 trace evidence

Read-only parsing of `tuning-r8.jsonl` found two complete three-finger drags:

```text
drag #0: commit pre-displacement centroid = (+0.250, -0.861) mm
         total drag vector               = (+26.24, -26.13) mm

drag #1: commit pre-displacement centroid = (-0.653, +0.722) mm
         total drag vector               = (-32.26, +30.31) mm
```

The second gesture drags the same direction back from the first gesture, so
its classifier displacement naturally points opposite the previous total drag
vector. The historical M15 `BeginDrag` path sent exactly that accumulated
centroid displacement after acquiring synthetic-left ownership. This directly
matches the user's report that a new drag begins offset in the reverse of the
previous drag vector. Because the same replay happens on every fresh commit,
it also explains why R8 could show the problem on the first drag after startup.

Earlier R7 evidence had already shown fresh tracking-id sets and long empty
gaps between drags, ruling out an un-reset cross-interaction centroid anchor.

### M19-only repair

`ThreeFingerDragConfig` now carries an internal
`stable_reference_motion` policy bit. Its default is `false`, preserving
M15-M18. `M19Profile` explicitly enables it.

In M19 stable-reference mode:

- the three-finger centroid still decides whether the configured commit
  threshold has been crossed;
- the commit action is `ArmDrag`, which establishes a reference baseline and
  emits neither classifier motion nor synthetic-left down;
- subsequent motion comes from one stable tracking-id reference finger;
- the first post-commit delta that survives fidelity/dead-zone processing is
  the first real drag output; that semantic frame contains `ButtonDown(Left)`
  before `PointerMove`;
- if the reference finger lifts while another original contact remains, a new
  reference is selected and re-baselined with zero motion on that switch frame;
- clean `3 -> 2 -> 1 -> 0` tails remain under drag ownership until the original
  cluster is empty; no remaining original finger leaks into ordinary pointer
  or two-finger scroll policy;
- a new/replacement tracking id remains fail-closed and ends the drag;
- the existing M19 drag-only fidelity ceiling and libei logical-frame batching
  remain in place.

The historical `BeginDrag { accumulated centroid delta }` path remains active
for M15-M18, and their existing public integration tests continue to pass.

### Dedicated regressions

```text
commit_rebaselines_instead_of_replaying_classifier_displacement       PASS
reference_finger_lift_rebaselines_without_position_jump               PASS
committed_drag_keeps_staggered_tail_until_cluster_is_empty             PASS
m19_three_finger_commit_discards_classifier_motion_and_defers_press    PASS
M15 public drag integration suite                                      4 PASS
```

The M19 profile test also asserts `stable_reference_motion=false` for M18 and
`true` for M19, preventing this live repair from silently rewriting earlier
profile contracts.

### Final evidence after Re-review 9

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
touchpad-core unit suite                                        271 PASS
touchpadctl unit suite                                          119 PASS
public CLI integration                                           22 PASS
settings-full.json strict validation                            PASS
settings.json strict validation                                 PASS
```

The chained gate command completed with exit code 0. This repair used only
read-only inspection of `tuning-r8.jsonl`, source comparison and offline/fake
tests; it did not trigger a real takeover, real pointer/button emission or KDE
desktop action.

### Re-review 9 verdict

**APPROVED FOR BOUNDED USER LIVE RE-TEST.** R9 targets the specific per-gesture
origin shift demonstrated by R8 rather than further tuning gain or compositor
refresh assumptions. The next live test should repeatedly drag one icon back
and forth, including deliberate reference-finger lifts, and verify that every
fresh drag starts under the current pointer with no classifier-vector jump.

## Re-review 10 — Fast-flick drag-start hardware-frame barrier (2026-08-24)

### Trigger

The R9 live run removed the visible offset during slow three-finger drags, but
fast flicks could still start a newly dragged icon displaced in the direction
of the new flick (which is the reverse of the previous vector during repeated
back-and-forth testing).

### R9 evidence and rejected hypotheses

Read-only replay analysis of `tuning-r9.jsonl` found 20 contact clusters that
reached three fingers. The classifier crossed the 0.8 mm drag threshold with
fresh per-cluster reference state, and the first post-commit reference delta
arrived roughly one input frame later (about 6 ms in the observed samples).

The staggered touchdown path was specifically checked because the current
`linux-3-finger-drag` implementation buffers a fresh touch before deciding
whether it is an ordinary pointer or a drag. In R9, however, the first finger
moved at most **0.172 mm before the second finger landed**, far below this
project's 1.0 mm ordinary-pointer commit threshold. Therefore the residual R9
offset is not explained by a one-finger pointer move leaking before the full
three-finger cluster exists.

The M19 classifier/replay separation also remains intact: the commit frame is
still motion-free, stable reference-finger tracking is fresh per drag, and a
reference replacement still re-baselines with zero delta.

### Corrected reference-project comparison

Re-review 8 treated `ButtonDown(Left) + PointerMove` in one libei hardware
frame as desirable. A direct comparison with the current
`lmr97/linux-3-finger-drag` runtime shows an important difference that was
missed in that review:

- `drive_drag()` emits `MouseDown` before the first non-zero `MouseMove`;
- `MtProxy::apply()` applies those outputs synchronously and in order;
- `VirtualTrackpad::mouse_down()` writes `BTN_LEFT` followed by its own
  `SYN_REPORT`;
- `VirtualTrackpad::mouse_move_relative()` then writes `REL_X/REL_Y` followed
  by a second `SYN_REPORT`.

So the reference implementation establishes drag ownership at the stationary
cursor position in one hardware frame, then applies the first relative motion
in the next hardware frame. It does **not** merge the press and first move
into one hardware frame.

This distinction is most relevant during a fast flick, where the first real
post-commit reference delta can already map to several logical pixels. Giving
KWin a separate press frame removes ambiguity about whether drag-surface
acquisition/hit-testing observes the pre-motion or post-motion pointer
position.

### M19-only repair

`ArbiterSink::frame()` now recognizes only the exact first-motion shape of an
M19 stable-reference three-finger drag:

```text
ButtonDown(Left), PointerMove(...)
```

when `stable_reference_motion == true` and the three-finger drag state is
`Dragging`. That exact decision is submitted to the output sink as two
contiguous hardware-frame segments:

```text
frame A: ButtonDown(Left)
frame B: PointerMove(...)
```

All other decisions retain the existing `submit_frame()` behavior. In
particular:

- M15-M18 keep their historical path;
- ordinary one-finger pointer behavior is unchanged;
- M19 one-finger tap-and-drag still submits its first
  `ButtonDown + PointerMove` together;
- later three-finger drag motion is unchanged;
- release behavior and staggered-tail ownership are unchanged.

The split submission preserves accepted-prefix accounting. If the press frame
commits but the following motion frame fails, the adapter reports global
`accepted_prefix = 1`, reconciles synthetic-left as delivered/held, enters the
normal fail-stop state, and therefore still owes exactly one matching release.

### New regressions

```text
m19_stable_three_finger_first_motion_commits_press_before_motion_frame PASS
m19_one_finger_tap_drag_keeps_press_and_motion_in_same_sink_frame      PASS
split_drag_start_motion_failure_reports_global_prefix_and_keeps_left_owed PASS
```

The first regression records `OutputSink::submit_frame` boundaries, not only
semantic event order, so it fails if a future refactor silently merges the
M19 press barrier back into the first motion frame. The second protects the
already-restored tap-and-drag behavior from this targeted change. The third
locks down cleanup correctness across the newly introduced two-segment
submission.

### Final evidence after Re-review 10

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
```

Both debug and release workspace gates completed with exit code 0, and the
release `touchpadctl` binary was rebuilt. No real takeover or real desktop
pointer/button output was triggered during this repair.

### Re-review 10 verdict

**APPROVED FOR R10 FAST-FLICK LIVE RE-TEST.** The remaining R9 symptom is now
targeted at the only verified output-boundary mismatch found against the
reference implementation. Live testing should emphasize rapid alternating
flicks on the same desktop icon; slow drags should remain unchanged. If the
fast-flick origin offset survives this barrier, the next investigation should
capture compositor-visible cursor/drag-surface timing rather than changing
classifier, reference-finger or gain logic again without new evidence.

## Re-review 11 — Fast touchdown entry debounce (2026-08-24)

### New live discriminator

The user isolated a much stronger trigger condition after R10 work:

- placing the three fingers one after another and then dragging is stable;
- dropping all three fingers rapidly and immediately flicking still reproduces
  the origin-shift bug.

This changes the likely fault layer. The failure is correlated with the
three-finger *entry transient*, not with steady-state drag motion.

### Reference-project comparison

Current `lmr97/linux-3-finger-drag` keeps a dedicated fresh-touch entry buffer.
Its default `entryDebounce` is 50 ms and is measured from the beginning of the
contact cluster, specifically to avoid classifying asynchronous 2 -> 3 -> 4
finger touchdown while the hand is still landing. M19 had adopted the stable
tracking-id reference motion, but did not yet carry this entrance-stability
layer.

### Targeted M19 repair

`ThreeFingerDragConfig` now has an optional `entry_debounce`. Historical
M15-M18 configs leave it at zero, preserving their existing behavior. M19 sets
it to 50 ms.

For M19:

1. the debounce clock begins on the first non-empty contact of the cluster,
   not when the third finger appears;
2. if three fingers arrive before the 50 ms window closes, the three-finger
   candidate owns/suppresses those frames but cannot commit;
3. when the window closes, all displacement accumulated during touchdown,
   hand spread and initial settling is discarded;
4. the classifier anchor is re-established at the settled position;
5. the existing 1 mm drag threshold and stable-reference motion then operate
   normally from subsequent frames.

Because timing begins on the first finger, deliberately staged 1 -> 2 -> 3
placement usually pays the entire debounce before the third finger arrives;
that already-good path therefore receives no additional fixed 50 ms delay from
the third-finger touchdown.

Tap-and-drag, later drag motion, release/tail ownership, M15-M18 profiles and
the R10 press/motion hardware-frame split are unchanged.

### New regressions

```text
fast_three_finger_entry_discards_touchdown_motion_until_debounce_expires PASS
staged_fingers_pay_entry_debounce_before_third_finger_arrives            PASS
m19_stable_three_finger_first_motion_commits_press_before_motion_frame   PASS
m19_one_finger_tap_drag_keeps_press_and_motion_in_same_sink_frame        PASS
split_drag_start_motion_failure_reports_global_prefix_and_keeps_left_owed PASS
```

The first test injects a 4 mm fast flick during the entry window and verifies
that it cannot arm or move the drag; after 50 ms the classifier reanchors and
only new motion can commit. The second confirms that staged fingers do not
restart a fresh 50 ms timer when the third finger lands.

### Final evidence after Re-review 11

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
```

One public M11 fake-backed CLI integration test transiently aborted during the
first combined debug run; rerunning that exact test passed immediately, and a
subsequent complete debug + release workspace run finished with exit code 0.
No real takeover/output was triggered during this repair.

### Re-review 11 verdict

**APPROVED FOR R11 RAPID-TOUCHDOWN LIVE RE-TEST.** The implementation now
matches the reference project's missing entrance-debounce idea while keeping
the user's already-good staged-finger path fast. The next live test should
explicitly compare staged placement against simultaneous three-finger flicks.

## Re-review 12 — Remove the second post-debounce classifier (2026-08-24)

### R11 live result and trace comparison

R11 materially improved the rapid three-finger experience, but the user could
still reproduce the old origin-offset bug intermittently. The new trace was
compared directly against R9/R10 rather than tuning the 50 ms constant by
feel.

`tuning-r11.jsonl` contains 13 contact clusters that reach three fingers. For
the rapid entries, the first-to-third-finger landing interval is typically
24-49 ms. R11 also contains one `SYN_DROPPED` at trace startup, but it occurs
about 12.19 seconds before the first three-finger cluster. R9/R10 contain no
drop. The offline replay failure on R11 is therefore explained by the trace's
unresolvable startup discontinuity and is not temporally correlated with the
drag bug.

The pre-third-finger lower-policy leakage hypothesis was re-checked against
the actual R11 data:

- maximum one-finger displacement before the second finger: 0.224 mm;
- maximum two-finger centroid displacement before the third finger: 0.486 mm.

Both remain below the current 1 mm one-/two-finger commit thresholds, so this
is not the R11 residual trigger. The first post-commit frame was also compared
across all three physical fingers. Their displacement vectors are highly
coherent (roughly 0.97-1.00 vector coherence), ruling out a reference-finger
tracking-id jump: the large first delta is genuine whole-hand motion.

### Why increasing 50 ms is the wrong fix

The R11 implementation discarded touchdown motion, then re-established a new
three-finger classifier anchor at debounce expiry and required another drag
threshold crossing (`feel.drag.commit_threshold_mm = 0.8`) before `ArmDrag`.
That second classifier moves drag acquisition later into the high-speed part
of a flick.

Trace simulation of the actual R11 path found the first real drag delta at a
median of about 1.091 mm (about 14.3 logical pixels at the M19 minimum-gain
scale), with a maximum of about 2.004 mm (about 26.3 px). Increasing the entry
window to 75-100 ms does not solve this: on the same trace it generally moves
the first emitted sample even deeper into the fast-motion phase and increases
the initial delta.

This is therefore a sequencing bug, not evidence that `50 ms` simply needs a
larger value.

### Verified reference semantic missed by R11

The current `lmr97/linux-3-finger-drag` implementation resolves a fresh touch
at `touch_start + entry_debounce`. If the touch has held at exactly three
fingers, it commits the drag immediately, discards the buffered touch frames,
and establishes the drag reference baseline on that commit frame. It does
**not** start a second movement classifier after the entry window.

Its first later reference-finger movement then produces `MouseDown` followed
by `MouseMove`. R11 copied the entry window but accidentally retained M15's
second displacement classifier after it, which is the newly verified mismatch.

Simulating the reference-style direct commit on the R11 raw contacts reduces
the first post-arm delta median from about 1.091 mm to about 0.336 mm (about
4.4 px). This is a substantial reduction without increasing entry latency.

### R12 targeted repair

For the M19 fast-entry path only:

1. the 50 ms clock still begins with the first contact;
2. while the window is open, three-finger touchdown/settling motion remains
   suppressed and is never replayed;
3. when the window resolves with a stable three-finger cluster, M19 now moves
   directly to `Dragging`, baselines the stable reference finger at the
   current coordinates, and returns `ArmDrag`;
4. there is no second 0.8 mm movement threshold after debounce;
5. `ArmDrag` still emits neither left press nor pointer motion;
6. the first subsequent real reference movement remains responsible for the
   synthetic left press and first pointer motion, preserving the R10 hardware
   frame barrier;
7. deliberately staged fingers whose third contact arrives after the debounce
   window retain the historical threshold path and therefore keep the already
   good staged-placement feel.

M19 also records whether any post-arm drag motion has occurred. A short,
stationary three-finger contact that was directly armed at debounce expiry can
still resolve as the existing semantic three-finger tap when all contacts lift;
this preserves a project feature that the reference implementation itself does
not provide.

### R12 regressions

```text
fast_three_finger_entry_discards_touchdown_motion_until_debounce_expires PASS
debounced_stationary_three_finger_contact_can_still_tap                  PASS
staged_fingers_pay_entry_debounce_before_third_finger_arrives            PASS
m19_stable_three_finger_first_motion_commits_press_before_motion_frame   PASS
m19_one_finger_tap_drag_keeps_press_and_motion_in_same_sink_frame        PASS
split_drag_start_motion_failure_reports_global_prefix_and_keeps_left_owed PASS
```

The partial-submit regression was deliberately retained: after direct arm,
if the split press frame succeeds but the immediately following first-motion
frame is rejected, the adapter still reports global `accepted_prefix = 1`,
remains faulted, and owes exactly one synthetic-left release.

### Final evidence after Re-review 12

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
```

The combined full gate finished with exit code 0 and rebuilt the release
`touchpadctl`. No real device takeover or desktop input was triggered while
performing this repair.

### Re-review 12 verdict

**APPROVED FOR R12 FAST-FLICK LIVE RE-TEST.** The R11 evidence argues against
increasing the debounce duration. R12 instead removes the verified extra
post-debounce classifier and aligns fast-entry drag commitment with the
reference state machine while retaining M19's tap and staged-finger behavior.

## Re-review 13 — R12 residual bug retained / timing-unification investigation (2026-08-24)

### R12 live result

The R12 fast-entry repair reduced the reproduction rate of the original
three-finger drag-origin offset very substantially, but did **not** eliminate
it. The user can still reproduce the same class of failure intermittently.
Therefore the issue remains OPEN: R12 is a strong mitigation and removes one
verified sequencing error, but it must not be recorded or treated as a full
fix.

The current evidence boundary is:

- staged one-by-one three-finger placement is still the most reliable path;
- simultaneous/rapid three-finger entry is now much more reliable than R10/R11;
- the residual failure is rare enough that further blind changes to the
  50 ms entry debounce are not justified by the existing traces;
- R11/R12 evidence already ruled out the earlier reference-id jump hypothesis
  for the measured fast-entry samples and showed that simply increasing the
  debounce can push acquisition further into the high-speed phase.

### New timing-consistency question

Before making another three-finger-specific timing change, review the M19
tap-and-drag release/follow-up timing together with the three-finger entry and
release timing. The user specifically requested that these related drag
interactions feel internally consistent rather than exposing visibly
different grace periods for equivalent "finger leaves / drag ownership should
settle" transitions.

This is an investigation requirement, not yet a blanket instruction to reuse
one numeric constant everywhere. The next implementation must first separate:

1. **gesture-entry debounce** (classification before ownership),
2. **tap-drag follow-up / release grace** (ownership continuity across a
   brief lift), and
3. **three-finger drag end / staggered-lift ownership** (ending an already
   committed drag).

Only timings with the same interaction semantics should be unified. If the
current M19 tap-drag release/follow-up interval is semantically the same kind
of ownership grace as the three-finger drag's release settling interval, the
profile should derive both from one documented M19 timing constant and lock
that relationship down with regression tests. Entry debounce must remain a
separate classifier concept unless evidence shows otherwise.

### macOS behavior investigation required before the next gesture-policy change

The next design pass must also investigate how macOS allows three-finger
dragging while Mission Control / desktop-space switching remains available
without ambiguous ownership. The investigation should distinguish Apple
settings/UI behavior from implementation inference and answer at least:

- whether enabling Accessibility three-finger drag changes the finger count
  used for Mission Control / switching Spaces;
- whether macOS resolves this primarily by remapping system gestures to four
  fingers, by gesture-mode exclusivity, by temporal/motion classification, or
  by a combination of those mechanisms;
- how drag ownership is latched once committed and what events are still
  eligible to become workspace gestures;
- which parts can be reproduced safely in this project's M18/M19 gesture
  router without introducing delayed pointer motion or accidental desktop
  switching.

No new claim of full three-finger-drag qualification should be made until the
R12 residual is reproduced or ruled out under the next instrumented build.

### macOS findings: avoid three-finger drag / workspace arbitration by finger-count ownership

Apple's current public gesture documentation presents the conflict-free
combination directly: Three-Finger Drag owns three-finger translational drag,
while Mission Control, App Expose, and horizontal full-screen-app / desktop
switching are shown as four-finger swipes. Trackpad settings expose the system
gestures as configurable gesture choices, and other Apple help pages describe
some of those system swipes as using three *or* four fingers depending on the
configuration/version. Apple does not publish the private recognizer/state-
machine implementation, so internal priority ordering must not be asserted as
fact.

The strongest safe design inference for this project is therefore:

1. when `three_finger_drag_enabled` is true, translational three-finger motion
   belongs exclusively to drag/tap classification;
2. Mission Control / workspace switching should default to four-finger
   gestures rather than compete with three-finger drag on direction or
   velocity;
3. once a three-finger drag is armed/committed, that contact cluster remains
   drag-owned until its release policy completes; it must not be re-routed
   mid-cluster into a workspace swipe;
4. users may still expose three-finger workspace gestures only when
   three-finger drag is disabled, or if a future settings UI explicitly
   resolves the conflict rather than silently allowing two owners.

This matches M18/M19's intended routing model better than a single recognizer
trying to distinguish "drag left" from "switch desktop left" after motion has
already begun: those gestures are geometrically identical and cannot be made
reliably conflict-free by a displacement threshold alone.

### macOS findings: tap-drag and three-finger drag do not use one identical lift timer

Apple's Pointer Control documentation makes an important semantic distinction:

- **Without Drag Lock** (tap-and-drag): after the dragging finger lifts, the
  item remains draggable for a short fraction of a second so the user can
  reposition the finger near the edge of the trackpad; a tap cancels that
  continuation immediately.
- **Three-Finger Drag**: dragging stops when the three fingers lift.

Therefore macOS fidelity argues **against** reusing the 50 ms three-finger
*entry debounce* as the M19 tap-drag follow-up/release timer merely to make the
numbers identical. They are different state-machine concepts, and Apple's
user-visible behavior also treats the two drag styles differently on lift.

Current M19 has `M19_TAP_DRAG_GAP = 180 ms`, which is the allowed gap between
the qualifying tap and the follow-up contact that may become tap-drag. A
committed M19 tap-drag already releases synthetic left immediately on its clean
Ended frame because sticky drag lock is disabled. Three-finger drag likewise
ends on the empty contact cluster. The existing 50 ms
`M19_THREE_FINGER_ENTRY_DEBOUNCE` is only pre-ownership classification time.

The next timing cleanup should therefore use semantic names and relationships,
not one shared numeric constant:

- `three_finger_entry_debounce`: keep separate; classifier-only;
- `tap_drag_follow_up_gap`: keep separate; this is the pre-drag continuation
  window after a tap;
- if live testing shows either committed drag needs a brief lift/reposition
  grace, add an explicit `drag_release_grace` state rather than overloading
  either of the above constants. For strict macOS Three-Finger Drag fidelity,
  the three-finger release grace should remain zero unless hardware evidence
  demonstrates that the apparent immediate release needs a tiny debounce for
  staggered physical liftoff.

### Routing recommendation for the next M19 pass

Adopt an explicit finger-count ownership rule at settings/profile construction:

```text
three_finger_drag_enabled = true
    three-finger translation -> drag/tap owner only
    four-finger horizontal   -> previous/next workspace
    four-finger vertical     -> overview / expose-style actions

three_finger_drag_enabled = false
    three-finger swipes may be user-bindable again
```

This removes the macOS-style conflict structurally before runtime arbitration,
leaving the recognizer to solve only the real ambiguity inside three-finger
drag itself (tap vs drag, entry transient, staggered lift), rather than also
trying to infer user intent between two identical three-finger translations.
