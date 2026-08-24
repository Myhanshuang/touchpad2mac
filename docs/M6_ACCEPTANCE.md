# M6 Acceptance — KDE Wayland Output Backend Qualification

Status: **implemented, pending external review**. The 2026-08-16 reviews
(R1–R6 blocking findings + required cleanup, then R7–R10 in re-review 1,
R11 in re-review 2, R12 in re-review 3, R13 in re-review 4) have been
repaired and regression tests added; this document reflects the repaired
state. M6 must not be
considered approved until an independent reviewer re-runs the gates below,
executes `output-probe --emit` on the current KDE Wayland session, and
measures the results. The backend is **`experimental/unqualified`** until
that measurement happens; it must not be used as a takeover default
(PHASE2_PLAN.md §5 M6).

This document strictly separates:

1. **Automated tests** — run everywhere, no portal/display/session
   bus/libei/hardware/root needed.
2. **Environment probing** — read-only observations of the current session.
3. **Interactive validation (not yet performed)** — the reviewer-run
   `--emit` measurement that decides qualification.

## 1. Automated tests (no Wayland, no portal, no libei, no hardware)

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The workspace now has **496 tests** (368 at M5 + 128 in M6), 0 failed.
Coverage added by the re-review repairs:

- **R1 (sound FFI)**: the libei FFI module is crate-private; the handles are
  non-`Copy` RAII owners released exactly once by `Drop`; `Libei` methods take
  borrowed handles; `ei_event_get_seat`/`get_device` return lifetime-bound
  views. Tests: owner `Drop` calls unref exactly once, null-handle drop is a
  no-op, owners move but never copy.
- **R2 (real Ctrl-C/SIGTERM)**: `output-probe --emit` (and `record`) install
  the controlled termination handler (command classification tested); the
  emit path observes the process-lifetime stop static. Tests: a **real**
  SIGINT and SIGTERM delivered via `libc::raise` map to exit 8 with nothing
  emitted, and guard teardown restores dispositions/resets the stop state
  (touchpadctl integration tests); the EIS handshake is cancellation-aware
  (`prepare_cancellable`); the portal waits are bounded and their delay
  before cleanup is documented.
- **R3 (post-handshake pump)**: `Transport::pump` drains server events
  around emission frames; device pause/removal, seat removal, and disconnect
  after a successful handshake transition the sink to `Interrupted`, reject
  subsequent output with no later wire event, and still run the ordered
  cleanup. Tests script these events after the handshake, prove no later wire
  event, and cover pump drain semantics and ignore-other-device pauses.
- **R4 (prepare cleanup preservation)**: `prepare` failures now return a
  `PrepareFailed { primary, cleanup }` composite that preserves the primary
  category/exit precedence and carries the cleanup diagnostics. Tests
  fault-inject every preparation stage combined with session-close and
  transport-disconnect failures.
- **R5 (per-axis scroll stop)**: scroll activity is tracked per axis; only
  axes that received nonzero deltas are stopped. Tests: x-only, y-only,
  two-axis, zero-delta, repeated lifecycle, partial send, and forced release.
- **R6 (MSRV)**: the workspace declares `rust-version = 1.87` — the real
  minimum of the locked graph (zbus 5.19/zvariant declare 1.87); the manifest
  no longer claims 1.85.
- **R7 (owners pin the loaded library)**: every libei owner handle
  (`EiContext`/`EiSeat`/`EiDevice`/`EiEvent`) holds its own `Arc` to the
  `libloading::Library`, so dropping the `Libei` loader cannot unload the
  library while any owner exists — the `unref` function pointer is valid for
  the owner's whole lifetime by construction. Tests prove the owner alone
  keeps the library pinned after the loader's reference is dropped (via the
  guard's `Arc` strong count) and still releases exactly once; the R1
  ownership tests now use a thread-local counter so they are race-free under
  parallel execution.
- **R8 (native queue discipline)**: `NativeTransport` is generic over a
  `NativeFfi` seam (the exact libei surface it uses; the real `Libei`
  implements it). `wait_event` now delivers already-fetched events from a
  native pending queue first, drains libei's internal queue with
  `ei_get_event` **before** polling, and drains the **entire** internal queue
  after every `ei_dispatch`; `pump` dispatches unconditionally (flushing
  queued outgoing libei data) even when the fd reports no readiness. A
  scripted-FFI test seam below raw FFI proves two or more events from one
  dispatch are all surfaced even when the following poll reports no
  readiness, that the pending queue is consumed before polling, and that
  `pump` flushes with no readable fd.
- **R9 (zero-axis ScrollEnd)**: an explicit `ScrollEnd` with no active axis
  is a local lifecycle marker — no wire `scroll_stop(false, false)` and no
  frame (analogous to `ScrollBegin`); a fully-zero `ScrollDelta` is likewise
  local-only. Regression test: a direct `begin → zero delta → end`
  interaction emits no scroll wire event and no frame, and the per-axis state
  resets for the next interaction.
- **R10 (outcome preservation)**: the emit orchestration is factored into
  `emit_pattern_with` with injected probe/portal/transport factories and
  returns the **exact** successful `EmitOutcome` from the pattern run
  (`steps_emitted`, `wire_events`, `skipped`, `capabilities` survive the
  ordered cleanup). Tests drive the real orchestration (real
  `PortalOutputSink` + `run_pattern` + cleanup) through fake portal/transport
  implementations — not `FakeDesktopOutput`, which bypasses the code — and
  prove nonzero counts and skipped capabilities survive cleanup, a server
  interruption stays the structured primary failure, and a cleanup failure is
  reported without losing the pattern result.
- **R11 (deferred mapping at delivery order)**: `NativeTransport`'s pending
  queue holds the **raw owned `EiEvent`s**, not already-mapped
  `TransportEvent`s; `map_event` (and its seat/device/resumed side effects)
  runs exactly when each event is popped for delivery, so a single dispatch
  containing several lifecycle events applies each event's state at the point
  the caller observes it. A queued `DevicePaused`/`DeviceRemoved`/
  `SeatRemoved`/`Disconnect` can no longer mutate transport state before the
  earlier `DeviceResumed`/`SeatAdded` is delivered. Native-adapter regressions
  (scripted FFI below raw FFI) prove, between successive `wait_event` calls,
  that a seat stays bindable until its queued removal is delivered, a resumed
  device stays usable (`start_emulating` succeeds) until a queued
  pause/removal is delivered, emission is rejected after the pause/removal
  delivery, and a queued disconnect becomes terminal only when delivered
  without losing the events queued before it.
- **R12 (object-path-safe portal tokens)**: the live `--emit` attempt exited
  9 with `internal error: Invalid object path` because `next_token`
  generated `m6-<pid>-<counter>` and the portal embeds the token as the
  **last element of the request/session handle object path**, which cannot
  contain `-`. Repaired: tokens are generated from the D-Bus
  object-path-safe alphabet (`[A-Za-z0-9_]` — the exact charset
  xdg-desktop-portal's `xdp_is_valid_token` accepts) as `m6_<pid>_<counter>`;
  the predicted request path
  (`/org/freedesktop/portal/desktop/request/<sender_component>/<token>`,
  sender component = unique name with the leading `:` stripped and `.` →
  `_`, exactly as `xdp_request_init_invocation` computes it) is validated
  with **zvariant before** the `Response` match rule is registered, failing
  with a structured `InvalidPortalPath` whose message names the kind, the
  constructed path, the sender component and the token instead of a
  context-free `Invalid object path`. `CreateSession` now supplies a
  **distinct** `session_handle_token` (validated against the predicted
  session path) alongside the request `handle_token`, per the RemoteDesktop
  spec's two documented option keys; the synchronous `ConnectToEIS` no
  longer adds a `handle_token` (its options contract documents none).
  Tests (pure, no live portal/session bus): every generated token is a
  valid object-path element and unique (10 000 per generator + cross-pid
  disjunction), every predicted request/session path for generated tokens
  is valid and follows the portal naming convention, an invalid token
  fails with the path-construction diagnostic, `CreateSession` options
  carry exactly the two distinct safe tokens, and the per-method options
  contract matches the spec (request methods never carry a session token;
  `ConnectToEIS` options stay empty).
- **R13 (`CreateSession.session_handle` is wire type `s`, not `o`)**:
  the installed
  `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml`
  declares the `CreateSession` response's `session_handle` as D-Bus string
  (`s`), noting the session handle "is an object path that was erroneously
  implemented as `s`. For backwards compatibility it will remain this
  type." The response decoding is factored into a **pure**
  `decode_create_session_response` (no live portal/session bus): it decodes
  the entry **as a string first**, then validates the string's contents as
  an `OwnedObjectPath` before storing `PortalSession`. The three failure
  classes keep distinct, contextual diagnostics: missing key (names the
  absent `session_handle`), wrong D-Bus value type (names the actual value
  and its D-Bus signature — including the *would-be-correct* `o` type,
  which the string-first wire ABI must reject), and syntactically invalid
  path contents (names the offending string). Tests: a valid path string
  (the exact installed v2 compatibility case) decodes; the missing-key,
  wrong-type (with value/signature context) and invalid-path cases fail
  with their distinct diagnostics. The code and docs no longer claim the
  response wire type is `o`.
- **Cleanup**: the real device type is queried via `ei_device_get_type` and
  physical devices (millimetre deltas) are rejected before claiming the
  logical-pixel mapping; the portal response wait races the async stream
  against a deadline so a timed-out request exits its helper thread instead
  of abandoning it; the workspace-local `.cargo-home` cache and `target`
  artifacts are removed and ignored; the `FakeTransport::disconnect` double
  assignment was verified as already single.

**No automated test constructs the real portal or the real libei transport**:
every test drives the fake seams ([`touchpad_desktop::FakePortal`],
[`FakeTransport`], [`FakeDesktopOutput`]), and the native transport's
event-loop algorithm is exercised **below raw FFI** over a scripted
`NativeFfi` seam (fake raw handles; no real library, fd, or emission — M6
re-review R8). The only libei surface a test touches is the side-effect-free
`Libei::load()` dlopen probe. No test opens, reads, records, or grabs any
`/dev/input` device, and no test emits real desktop input. The only real OS
surfaces exercised are side-effect-free: `sigaction`/`raise`,
nonexistent-path filesystem checks, the dlopen probe, the session-bus
reachability probe, and the real-signal regressions above.

## 2. Environment probing (read-only, current KDE Wayland session)

Observed on the review machine (side-effect-free; no session created, no
input emitted, `/dev/input` not touched):

- Session: `WAYLAND_DISPLAY=wayland-0`, `XDG_SESSION_TYPE=wayland`,
  `XDG_CURRENT_DESKTOP=KDE`, session bus reachable.
- Portal: `org.freedesktop.portal.RemoteDesktop` **interface version 2**
  (introspected on `org.freedesktop.portal.Desktop` at
  `/org/freedesktop/portal/desktop`), `AvailableDeviceTypes = 7`
  (keyboard|pointer|touchscreen), `ConnectToEIS` method present (v2 EIS-fd
  hand-off).
- libei: `libei.so.1` loadable (libei/liboeffis 1.6.0 installed; headers
  `/usr/include/libei-1.0/libei.h`).
- `touchpadctl output-probe` (dry-run) on this machine reports all of the
  above, the requested capabilities (relative pointer, primary/secondary
  button, pixel-precise smooth scroll), the eight `--emit` steps, and
  `backend state: experimental/unqualified`; exit 0.

## 3. Reviewer-run interactive validation (`--emit` measurement) — NOT YET PERFORMED

The backend is **not qualified** until the reviewer runs and measures:

### 3.1 Required run

```text
cargo build --workspace            # or: cargo run -p touchpadctl -- output-probe --emit
touchpadctl output-probe           # dry-run: confirm the report first
touchpadctl output-probe --emit    # real, bounded desktop emission
```

`--emit` prints a warning, a 3-second countdown (Ctrl-C cancels, exit 8),
then emits the fixed pattern: relative moves **+10 px**, **+50 px**,
**+200 px** (x-axis), a primary click (`BTN_LEFT` down/up), a smooth scroll
(begin, −120 px, −240 px, end), a secondary click (`BTN_RIGHT` down/up).
Total wire events ≤ 16. The scroll stop is **per-axis** (M6 re-review R5):
the probe scrolls only y, so only the y axis is stopped — the reviewer can
verify no spurious x-axis scroll-stop is generated. It then releases all
held state, disconnects, and closes the session. The reviewer must observe
the portal authorization dialog (KDE), approve it once, and also test
**cancelling** it in a second run to confirm the structured
`authorization-cancelled` result (exit 3, no panic). A **real Ctrl-C or
SIGTERM** during the countdown is handled by the installed termination
handler (installed for `--emit` exactly like `record`, M6 re-review R2):
the process returns exit 8 through the ordered cleanup instead of being
terminated by the kernel; the authorization/handshake waits are bounded
(Start ≤ 120 s, handshake ≤ 15 s), and a signal during the handshake aborts
promptly (exit 8, session released).

### 3.2 A/B measurement procedure (decides qualification)

1. **Relative-delta displacement.** For each delta (10, 50, 200 px), repeat
   the run **N ≥ 10** times and measure the actual on-screen pointer
   displacement (e.g. position the pointer at a screen ruler/grid, record
   before/after). Record mean and spread per delta.
   - If displacement ≈ delta for every delta and sample → relative motion is
     **not** re-accelerated/reinterpreted by the compositor for these deltas
     (candidate for `qualified` on motion).
   - If displacement consistently differs (scaled, non-linear across deltas,
     or jittery) → **unqualified**: the compositor (or libinput/KDE pointer
     acceleration) is still processing the deltas; do not claim relative
     motion avoids acceleration. Record the observed ratio.
2. **Pixel scroll.** During the scroll step, verify the scroll is **smooth
   (pixel-precise)** and that no discrete wheel-step conversion is visible;
   check KDE's "smooth scrolling" behavior in a scrollable window. Record
   whether the scroll distance matches −120/−240 px and whether a second
   compositor-side acceleration is evident.
3. **Button release.** After the pattern, verify no button remains logically
   held: no drag mode, no stuck menu after the right click, no selection
   state. Also confirm `release_all` idempotency by the exit 0 and the
   "emission complete" summary, and that the pointer/keys return to normal
   operation immediately (no residual grab).
4. **Cleanup after cancel/refusal.** Run `--emit` and cancel the dialog;
   confirm exit 3, no panic, and that the system pointer remains usable.
   (A prepare failure whose cleanup also fails is reported as a composite
   `PrepareFailed` preserving the primary exit code, M6 re-review R4.)
5. **Disconnect behavior.** With the session authorized, interrupt the
   process (Ctrl-C during the countdown is exit 8 via the installed
   termination handler; a `SIGTERM` during the short emission is also
   handled, M6 re-review R2). If the EIS server pauses/removes the device or
   disconnects mid-pattern, the adapter reports the structured failure
   (exit 5), rejects further output, and still runs the ordered release
   (M6 re-review R3); record what was observed.

### 3.3 Reviewer decision

Mark the backend `qualified` (for the measured capabilities only, with the
measurement table) or leave it `experimental/unqualified` (with the recorded
deviations). **Until this section is completed and recorded, no takeover
(M10) may use this backend as a default.**

## 4. Safety scope of M6 (what was and was not done)

- M6 does **not** open, read, record, or grab any physical `/dev/input`
  device; does **not** add takeover, pointer/scroll policy algorithms, tap,
  drag, gesture recognition, daemon/service behavior, autostart, or
  system-setting changes; does **not** automatically move the pointer, click,
  or scroll during tests or ordinary probe execution; does **not** create a
  virtual touchpad or expose raw contacts/finger counts (touch capability is
  never bound).
- The only `unsafe` is the crate-private libei FFI boundary with non-`Copy`
  RAII handle owners (released exactly once; no duplication, no double
  release, no use-after-release by construction — M6 re-review R1; every
  owner additionally pins the loaded library itself with its own `Arc`, so
  the library cannot be unloaded while an owner exists — M6 re-review R7);
  every other module is `#![forbid(unsafe_code)]` (the native transport is
  `#![deny(unsafe_code)]`, with the raw-handle construction confined to its
  scripted test seam below raw FFI).
- The real device type is queried (`ei_device_get_type`); only **virtual**
  devices (logical-pixel deltas) are used, and physical devices
  (millimetre deltas) are rejected before any unit mapping is claimed
  (M6 cleanup).
- Output preparation and authorization (`PortalOutputSink::prepare`) are
  designed to complete before any future `EVIOCGRAB` (M10 ordering
  invariant); M6 itself never grabs.
- MSRV: the workspace declares `rust-version = 1.87` (the locked graph's
  real minimum — zbus 5.19/zvariant declare 1.87; M6 re-review R6). Gates
  run on rustc/cargo 1.97.1; the 1.87 toolchain gate has not been
  independently run.
- No credentials are stored in any file, log, test, or fixture.
