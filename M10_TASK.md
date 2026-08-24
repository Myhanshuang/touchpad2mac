# M10 Task — Bounded, Fail-Open Live Takeover Slice

Date: 2026-08-17  
Status: implementation task; M9 is approved in `reviews/M9_REVIEW.md` Re-review 2  
Execution rule: dsh implements and runs only offline/fake-backed tests. The reviewer must approve the code before the user performs any live test.

## 1. Outcome and acceptance boundary

Implement the first bounded vertical slice:

```text
explicit evdev device (exclusive grab)
  → existing Type-B decoder/resync
  → approved M7–M9 Arbiter/ArbiterSink
  → prepared portal + libei streaming OutputSink
  → current KDE Wayland desktop
```

M10 is code-complete when the static/fake-backed gates and independent review pass. It remains **live-unqualified / pending user acceptance** until the user completes the 10-second, 60-second, then 300-second sequence. Do not run the takeover command during implementation or automated tests.

M6 proved the real portal/EIS/libei protocol path, but compositor-side relative-delta and pixel-scroll calibration was not recorded. Therefore takeover must never be a default and must require an explicit operator attestation that the M6 output calibration was performed. The attestation is not itself measurement evidence; documentation must say so and must not mark the backend or M10 live-qualified before the user records results.

## 2. CLI contract

Add a foreground-only command with an unambiguous contract, for example:

```text
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m10-linear-v1 \
  --max-duration-seconds N
```

Equivalent spelling is acceptable only if all properties below remain true:

- `DEVICE` and recorder `TRACE` are mandatory explicit paths.
- `--takeover`, exact confirmation text, output-qualification attestation, named versioned profile, and maximum duration are all mandatory and independently validated.
- Duration is an integer in `1..=300`; no zero, overflow, missing, repeated, or unlimited form is accepted. Initial M10 cannot run longer than five minutes.
- Unknown/repeated flags and use of takeover-only flags on other commands are usage errors before any device/output side effect.
- No daemon, fork/background mode, autostart, service file, persistence, config mutation, or system-setting write.
- Help must state that this grabs the physical touchpad, emits real desktop input, opens a portal authorization prompt, records raw input, is experimental, requires an external keyboard/mouse and a second-terminal `SIGTERM` escape route, and cannot promise cleanup after `SIGKILL`, kernel failure, or power loss.
- `record --grab` remains separately explicit and all existing command behavior remains compatible.
- The command installs the existing controlled SIGINT/SIGTERM handler; dry-run/non-live commands do not broaden their signal behavior.

## 3. Explicit versioned policy profile

Add one named profile, `m10-linear-v1`, whose every M7–M9 parameter is typed, finite, validated, and documented: one-finger linear pointer scale/commit threshold, tap/tap-and-drag/drag-lock timing and movement limits, two-finger 2D natural scroll scale/commit threshold, secondary tap, and buttonpad two-finger click.

The profile is a conservative bring-up profile, not a macOS-equivalence claim and not a production default. Do not read or copy KDE/libinput hidden/default values at runtime. Current system behavior is only the manual A/B baseline. Do not add acceleration, momentum, palm/thumb classification, pinch/rotate/swipes, Force Click, pressure, haptics, or later-milestone behavior.

## 4. Preparation order — grab is the final irreversible step

No raw event read and no `EVIOCGRAB(1)` may happen before all preconditions succeed. The observable order is:

1. Parse and validate the complete CLI contract with zero side effects.
2. Open and validate the explicitly named evdev device on its session fd, select the monotonic clock, but do not read or grab.
3. Probe and prepare a reusable streaming portal/libei output session. Require relative pointer, primary button, secondary button, and pixel-precise two-axis scroll capability. A fixed M6 probe pattern is not the streaming API and must not be replayed.
4. Construct the approved M7–M9 arbiter pipeline with `m10-linear-v1`.
5. Create the mandatory trace recorder from the exact validated descriptor and successfully write/flush its header; attach it before any raw event can reach the decoder.
6. Print the resolved device, trace, profile, negotiated capabilities, maximum duration, cleanup order, and escape instructions. Run a visible cancellable countdown of at least three seconds.
7. Re-check stop/cancellation and all readiness state. Only then issue exactly one `EVIOCGRAB(1)`, immediately before the bounded event loop.

Any failure/cancel before step 7 must issue zero grabs and zero semantic desktop events. A prepared output session must still be explicitly released/closed, and an opened device/recorder must still be finalized/closed in the ordered coordinator path with diagnostics preserved.

## 5. Streaming output boundary

Extend the M6 adapter with a reusable prepared streaming session that implements `touchpad_core::OutputSink` and exposes negotiated capabilities/readiness. Keep zbus/libei/native types inside `touchpad-desktop`; do not leak platform details into core.

- Preparation is cancellable and bounded exactly as M6.
- `submit` delegates through the already-reviewed portal sink and preserves synchronous accepted/rejected semantics.
- `release_all` remains idempotent and performs explicit semantic releases, transport disconnect, and portal session close.
- Server pause/removal/disconnect is a terminal output fault; after the first rejected semantic event no later wire output is allowed.
- Do not construct a virtual touchpad or forward raw contacts/finger count. Only resolved pointer/button/scroll events are emitted.
- Production code uses the real streaming factory; tests inject a fake session/factory and never connect to D-Bus, Wayland, portal, or libei.

## 6. Fallible frame bridge and no-late-output rule

The current Linux `FrameSink` callback is infallible while `ArbiterSink::frame` is fallible. Add a narrow takeover bridge/coordinator that stores the **first** arbiter/output failure, immediately stops accepting semantic work, and ignores all later frames from the same already-read evdev batch. The command must inspect that stored fault after every runtime step and begin shutdown. Do not silently log and continue, and do not replace the primary fault with a generic decoder error.

Preserve `ArbiterSink` accepted-prefix/faulted state so cleanup submits exactly the still-owed releases. Existing replay/record sinks and public decoder behavior must remain compatible.

## 7. Truly bounded event loop

The maximum duration must expire even when the touchpad produces no input. Polling only between blocking `read(2)` calls is insufficient.

Add a mockable bounded-readiness/timeout seam (or an equally strong design) so the live loop wakes at a short fixed quantum, checks an injected monotonic clock, signal stop, bridge fault, and deadline, then reads only when ready. Tests must use a fake clock/sys and no sleeps. The grab duration may exceed the configured limit only by the documented polling quantum; prefer no logical overshoot in the coordinator state.

Keep existing `EvdevRuntime::step()` M4/M5 semantics compatible. M10 needs a deferred-cleanup step path: fatal stream/decoder/recorder errors must stop new work but leave output, recorder, grab, and fd available to the unified M10 coordinator. Do not let the existing immediate runtime fail-open release the device before virtual output cleanup. Drop remains only a best-effort fallback.

## 8. One unified ordered shutdown

Every post-preparation exit—normal deadline, SIGINT/SIGTERM, output/arbiter fault, portal revocation, device EOF/unplug, poll/read error, decoder degraded or resync failure, recorder failure, grab failure, status-writer failure, or panic fallback—must converge on an idempotent coordinator:

1. Stop accepting raw/semantic work.
2. `ArbiterSink::release_all`: release owed virtual Left/Right and scroll lifecycle, then let the wrapped portal sink disconnect and close its session.
3. Finalize/destroy the recorder, preserving its finish result.
4. `EVIOCGRAB(0)` at most once.
5. Close the device fd exactly once even if ungrab failed.

For pre-grab failures the same order applies to resources that exist, with zero ungrab ioctl if no grab was acquired. A retry/repeated shutdown is a full no-op.

Return/report a structured outcome containing the primary stop/failure reason and **all** cleanup failures: every explicit virtual release, wrapped output cleanup, recorder finish, ungrab, close, and status-output failure. Do not flatten to only one string or let cleanup overwrite the primary. Exit-code precedence must be deterministic and documented; a controlled deadline/signal is reported as clean only when all required cleanup succeeded. Never claim SIGKILL cleanup.

## 9. Required automated tests

All M10 tests are fake-backed and must assert one shared timeline where ordering matters. At minimum cover:

- every missing/duplicate/invalid CLI opt-in; duration 0, 1, 300, 301, malformed and overflow; takeover flags rejected elsewhere;
- ordinary commands and takeover parse failures cause zero open/output/recorder/grab calls;
- device-open failure causes no output preparation/grab;
- output probe/authorization/capability/handshake failure: zero recorder events, zero grab, device closes;
- recorder create/header-flush failure after output ready: output releases before device close, zero grab;
- countdown cancel/signal/status-writer failure: output release → recorder finish → close, zero grab;
- exact success startup timeline: device open/validate → output ready → recorder header flush → countdown complete → grab → first read;
- max duration expires with an entirely idle device and leads to ordered cleanup; boundary at 1 and 300 seconds; fake clock, no sleep;
- SIGINT/SIGTERM and injectable stop during the loop;
- pointer, physical Left drag, tap-to-click, tap-and-drag/drag-lock, 2D natural scroll, secondary tap, and two-finger physical click travel through decoder → arbiter → output in order without raw contact leakage;
- one trace/replay-parity assertion for the same raw input used by takeover;
- first output rejection/partial accepted-prefix failure causes no later semantic/wire output from the same read batch and cleanup releases exactly the owed state;
- server interruption, device EOF/unplug, read/poll error, timestamp regression, decoder failure, `SYN_DROPPED` resync failure, recorder event failure, and grab failure;
- each primary failure combined with output release, recorder finish, ungrab and close failures; verify all diagnostics and stable exit precedence;
- multiple output explicit-release failures plus wrapped cleanup failure survive alongside later recorder/device cleanup failures;
- shutdown retry/idempotence and fallback Drop safety;
- capability missing refuses before recorder/grab;
- no test opens `/dev/input`, grabs a real fd, creates a real portal/libei session, emits desktop input, sleeps, or modifies system settings.

Use focused test helpers to keep the matrix readable. Avoid weakening earlier M1–M9 tests or changing approved semantics to make orchestration easier.

## 10. Manual acceptance document (write it, do not execute it)

Create `docs/M10_ACCEPTANCE.md` with:

1. Preconditions and exact build/probe commands.
2. M6 output-calibration table for repeated small/medium/large relative deltas and pixel scroll observations; this must be filled by the user before honestly passing `--output-qualified`.
3. Identification of the exact touchpad device and a warning to keep an external keyboard and mouse connected.
4. A second-terminal `SIGTERM` command and the configured duration as independent escape routes.
5. Exact 10-second command/checklist. Only after pass, the exact 60-second command/checklist. Only after pass, the exact 300-second command/checklist.
6. For each run: pointer, primary click/drag, tap, double tap, tap-and-drag/lock, vertical/horizontal/diagonal natural scroll, momentum explicitly absent, secondary tap/click, signal stop, deadline stop, trace replay, portal session closure, and post-exit restoration of the physical touchpad.
7. A result table with pass/fail, observed pointer/scroll scaling, duplicate/missing events, stuck button/scroll, cleanup messages, trace path, and deviations.
8. Explicit statements that M10 has no acceleration/momentum/palm/gesture/Force Touch/haptic claims and remains foreground/bounded.

The document must never embed credentials or suggest `sudo` as a generic solution. If permissions are missing, report the required group/udev access rather than changing the system.

## 11. Gates and handoff

Run and report:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Also scan changed non-generated text for credentials. Update `DESIGN_V2.md`, `MILESTONES.md`, CLI help/README/third-party documentation as applicable. Correct the stale M9 status to approved via `reviews/M9_REVIEW.md` Re-review 2.

Handoff must list changed files, exact startup/shutdown timelines, profile values, test counts, compatibility/API changes, dependencies/licenses/unsafe changes, unimplemented capabilities, and residual risks. Clearly separate automated validation from live validation. Stop after M10 implementation; do not begin M11.
