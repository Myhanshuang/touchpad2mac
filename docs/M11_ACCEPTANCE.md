# M11 Acceptance — Experimental One-Finger Pointer Fidelity (`m11-fidelity-v1`)

Status: **DRAFT — future user-run procedure only; NOT YET RUN and NOT
EXECUTED during implementation or review.** Nothing in this document has
been performed. The static/fake-backed offline gates were run during
implementation and review; the steps below are the remaining **user-run**
qualification gate for `m11-fidelity-v1`. Until the user completes every
stage below and records the results, M11 remains **experimental /
provisional / live-unqualified**. This document claims no macOS equivalence
and implies nothing about M12 (M12 work has not begun).

This document strictly separates:

1. **Automated/offline tests** — run during implementation and review; all
   fake-backed (no live takeover, no real device grab, no real portal/libei
   session, no emitted desktop input, no sleeping for timing, no
   system-setting change). They do not qualify M11 live.
2. **M6 output-calibration evidence** — the mandatory recorded measurement
   gate (Section 2) that must be complete before `--output-qualified`.
3. **M10 live acceptance** — the recorded ordered 10/60/300-second
   `m10-linear-v1` sequence (Section 3) that must be complete before any M11
   live run.
4. **M11 live acceptance** — this document's bounded, staged user-run
   procedure (Sections 4–7). It is proposed here and has NOT been run.

M11 is experimental, opt-in only, never the default, and makes **no macOS
equivalence claim**. Code completion does not confer live qualification
(M11_TASK.md §3, §14); M10 acceptance does not qualify M11; and M11
acceptance has no M12 implication (M12 has not begun).

## 1. Preconditions and exact build/probe commands

Build and run the offline gates:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Inspect the environment (non-emitting, read-only):

```text
touchpadctl devices
touchpadctl inspect /dev/input/eventN      # the exact touchpad node
touchpadctl output-probe                   # dry-run: portal/libei report
```

The M11 takeover command (do not run yet — see Sections 4–6):

```text
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m11-fidelity-v1 \
  --max-duration-seconds N
```

This is the existing bounded takeover contract unchanged (M11_TASK.md §4):
`DEVICE` and `TRACE` are mandatory explicit positional paths; all five
opt-ins remain mandatory and independently validated; the confirmation is
the exact non-interactive text `TAKEOVER`; `N` is an integer in `1..=300`;
missing, repeated, conflicting, malformed, overflow, unknown, or
takeover-only flags on another command remain usage errors before side
effects. M11 adds no flag; `m11-fidelity-v1` is a second accepted value of
the existing mandatory `--profile`, whose accepted set is exactly
`{m10-linear-v1, m11-fidelity-v1}` with `m10-linear-v1` mention-first. The
command is foreground-only and bounded: no daemon, no background mode, no
autostart, no service file, no persistence, no config mutation, and no
system-setting write. The grab may exceed `N` by at most the documented
polling quantum.

Before the first M11 run, verify the M11 banner is printed **before** any
device/output/recorder/countdown/grab side effect and contains all of:
experimental and uncalibrated; not the default; no macOS equivalence claim;
no live M11 validation has occurred; the M10 safety opt-ins and the
1..=300-second duration bound still apply.

## 2. M6 output-calibration evidence (must be recorded BEFORE `--output-qualified`)

`--output-qualified` is the **operator attestation** that the M6
relative-delta/pixel-scroll calibration was performed and recorded. It is
not itself measurement evidence — the recorded numbers are. M11 must not
weaken or bypass the M6/M10 gate (M11_TASK.md §3).

Record here (from `docs/M6_ACCEPTANCE.md` §3) before any M11 live run:

```text
M6 relative-delta calibration table recorded: YES / NO
  (10 px, 50 px, 200 px: mean/spread ≈ delta, not re-accelerated)
M6 pixel-scroll observation recorded:        YES / NO
  (smooth pixel-precise scroll, no wheel-step conversion, no second
   compositor-side acceleration)
M6 button-release / cancel-cleanup recorded: YES / NO
M6 backend decision:
  qualified / experimental-unqualified
  (M11 live acceptance requires honest delta evidence; if the deltas are
   scaled, non-linear across deltas, or jittery, do NOT pass
   --output-qualified — record the deviation and stop)
```

Reference to the recorded evidence (file/date):

```text
M6 evidence reference: ______________________________________________________
```

## 3. M10 live acceptance (must be completed and recorded BEFORE M11 live acceptance)

M11 is layered on the approved M10 interaction policy, and the M10/output
path must be live-qualified only through the recorded ordered sequence.
Before any M11 live run, the user must have completed and recorded
`docs/M10_ACCEPTANCE.md` §3 with `m10-linear-v1`:

```text
M10 Run 1 — 10 seconds (m10-linear-v1):   PASS / FAIL (recorded)
M10 Run 2 — 60 seconds (m10-linear-v1):   PASS / FAIL (recorded)
M10 Run 3 — 300 seconds (m10-linear-v1):  PASS / FAIL (recorded)
M10 signal stop (TERM + Ctrl-C):          PASS / FAIL (recorded)
M10 result table and observations:        COMPLETE / INCOMPLETE
```

M10 acceptance does **not** qualify M11: it is a precondition, not a
substitute. If any M10 run is not complete or not recorded, stop — do not
start M11 live acceptance.

## 4. M11 live acceptance — bounded staged procedure (proposed, NOT run)

The M11 live acceptance is **staged**: each stage is a separate bounded
takeover run with its own `--max-duration-seconds N` (`N` in `1..=300`), its
own trace path, and its own checklist. Stages must be executed in order; a
stage that fails stops the procedure (Section 6). Keep an external keyboard
and mouse connected and a second terminal ready (`kill -TERM <pid>`) for
every run. Confirm the exact touchpad device before each run
(`CIRQ1080:00 0488:1054 Touchpad`; KDE decimal vendor/product 1160/4180).

### 4.0 Before any M11 run

1. Confirm Sections 2 and 3 are complete and recorded.
2. Identify the device node:
   ```text
   touchpadctl devices
   touchpadctl inspect /dev/input/eventN
   ```
3. Keep an external keyboard and mouse connected (the touchpad is grabbed
   exclusively during each run).
4. Open a second terminal and keep it ready:
   ```text
   kill -TERM <pid>
   ```
   (`<pid>` = the takeover process id printed by the command). Ctrl-C in the
   takeover terminal is the primary route; the configured maximum duration
   is the automatic backstop.
5. Verify the M11 banner (Section 1) before approving the portal dialog.

### 4.1 Stage 1 — 10 seconds: low-speed precision / dead-zone behavior

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-01-deadzone.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 10
```

Checklist (pass/fail each):

- [ ] Portal authorization dialog appears; approve it once.
- [ ] Very small finger wiggles (sub-0.09 mm per sample, back and forth)
      produce **no** pointer jitter: the signed radial dead zone cancels
      oscillation algebraically and emits nothing.
- [ ] A slow, consistent one-finger movement is delayed until the
      accumulated signed displacement reaches the dead-zone radius
      (0.09 mm), then released **smoothly** (no jump, no burst).
- [ ] At slow speed the release uses the minimum gain (1.0): the observed
      on-screen displacement matches the M6-calibrated deltas (Section 2)
      at the M10 base scale (10 px/mm) — no re-acceleration of slow motion.
- [ ] The pointer does not drift when the finger is still.
- [ ] After the deadline (10 s) the process exits 0 (or the documented
      cleanup-failure code) and prints the cleanup status; the touchpad
      works normally afterwards.
- [ ] The trace `/tmp/m11-01-deadzone.jsonl` exists and replays:
      `touchpadctl replay /tmp/m11-01-deadzone.jsonl`.

### 4.2 Stage 2 — 30 seconds: normal pointer movement

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-02-normal.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 30
```

Checklist (pass/fail each):

- [ ] Moderate-speed one-finger movement tracks the finger with smooth,
      continuous pointer motion (no steps, no dropped segments, no
      double-counting).
- [ ] Slow-to-moderate segments scale near the M10 base scale
      (dead-zone radius excluded), consistent with the Section 2 deltas.
- [ ] No pointer jump/teleport and no drift during the run.
- [ ] Subpixel remainder behavior is invisible in practice: repeated small
      movements accumulate without loss (no per-move truncation drift).
- [ ] Exit 0 at the deadline; touchpad returns to normal; trace replays.

### 4.3 Stage 3 — 60 seconds: fast movement / gain

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-03-gain.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Checklist (pass/fail each):

- [ ] Fast flicks/quick strokes travel **farther** per millimeter than slow
      motion (gain rises above 1.0 toward the configured maximum 2.0 as the
      filtered velocity approaches 600 mm/s).
- [ ] The gain is **continuous**: no discrete speed bins, steps, or abrupt
      scale changes between slow and fast motion.
- [ ] The gain is **bounded**: the pointer never becomes uncontrollable or
      overshoots wildly (scalar capped at base × max_gain × tracking_speed).
- [ ] After a fast flick the pointer settles without oscillation or bounce.
- [ ] Exit 0 at the deadline; touchpad returns to normal; trace replays.

### 4.4 Stage 4 — 60 seconds: reversals / diagonals

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-04-reversal.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Checklist (pass/fail each):

- [ ] Rapid back-and-forth reversals below the dead-zone radius emit nothing
      (no jitter, no residual bias).
- [ ] A direction reversal after a release moves back smoothly with no dead
      spot or hysteresis hang.
- [ ] Diagonal movement (≈45°) keeps both axes consistent: the on-screen
      line stays straight (the scalar is isotropic — no x/y bias or skew).
- [ ] Reversing a diagonal reverses both axes proportionally.
- [ ] Exit 0 at the deadline; touchpad returns to normal; trace replays.

### 4.5 Stage 5 — 60 seconds: long idle / re-anchor

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-05-idle.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Checklist (pass/fail each):

- [ ] After the finger is lifted (or still) for ≥ 150 ms, resuming movement
      **re-anchors**: the gap-crossing displacement is discarded, so the
      pointer does **not** jump or lurch on resume.
- [ ] On resume there is no velocity spike (no burst of accelerated motion
      after idle).
- [ ] Leaving the touchpad idle for the whole run: the bounded loop still
      expires at the deadline (the maximum duration fires even with no
      input) and exits 0.
- [ ] No residual grab, no stuck state; touchpad returns to normal; trace
      replays.

### 4.6 Stage 6 — 60 seconds: click / tap / tap-drag / drag-lock / two-finger / button

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-06-inputs.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Checklist (pass/fail each) — all must behave exactly as in the M10
acceptance (M11 fidelity applies only to committed one-finger pointer
motion; candidate/tap/scroll ownership stays on pre-fidelity behavior):

- [ ] Physical primary click and click-drag (press and move with one
      finger).
- [ ] Tap-to-click produces a primary click.
- [ ] Double tap produces two click pairs.
- [ ] Tap-and-drag and drag lock behave (tap, then quickly touch again and
      drag; after lifting the drag stays locked; a qualifying tap releases).
- [ ] Two-finger vertical/horizontal/diagonal **natural** scroll is smooth
      (pixel-precise) in the natural direction.
- [ ] Two-finger tap produces a secondary (right) click.
- [ ] Buttonpad two-finger physical click produces a secondary click.
- [ ] Fidelity does not alter these behaviors: no double clicks, no stuck
      button/scroll, no pointer motion emitted from tap/scroll ownership.
- [ ] Exit 0 at the deadline; touchpad returns to normal; trace replays.

### 4.7 Stage 7 — 60 seconds: cleanup / signals

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-07-signals.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Checklist (pass/fail each):

- [ ] During the run, `kill -TERM <pid>` from the second terminal
      (independent escape route): clean exit 0 when all cleanup succeeded;
      touchpad restored.
- [ ] Ctrl-C in the takeover terminal on another run: same clean result.
- [ ] During the countdown, Ctrl-C cancels before the grab: exit 8, nothing
      was grabbed, no desktop input was emitted, the prepared output session
      was released, the recorder finalized, the device closed.
- [ ] After every stop the physical touchpad works normally (no residual
      grab, no stuck button/scroll) and the portal session closed (the KDE
      authorization indicator cleared).
- [ ] No duplicate or missing events; no stuck scroll; trace replays.

### 4.8 Stage 8 — 60 seconds: regression against `m10-linear-v1`

Run the same bounded motion patterns with the baseline profile and confirm
the M10 path is unchanged:

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-08-m10regress.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m10-linear-v1 --max-duration-seconds 60
```

Checklist (pass/fail each):

- [ ] The `m10-linear-v1` run reproduces the recorded M10 acceptance
      behavior (Section 3): same pointer scaling, same
      tap/drag/scroll/button results.
- [ ] `m10-linear-v1` remains output-compatible: the fidelity-disabled path
      follows the M10 linear branch and does not pass through M11 fidelity
      logic (M11_TASK.md §2).
- [ ] M11 adds no flag and no behavior change to the M10 command contract.
- [ ] Exit 0 at the deadline; touchpad returns to normal; trace replays.

## 5. Result table

| Stage | Run | Pass/Fail | Key observations | Deviations | Trace path |
| --- | --- | --- | --- | --- | --- |
| 1 dead-zone/low speed | 10 s | | | | |
| 2 normal movement | 30 s | | | | |
| 3 fast/gain | 60 s | | | | |
| 4 reversals/diagonals | 60 s | | | | |
| 5 long idle/re-anchor | 60 s | | | | |
| 6 click/tap/drag/scroll/buttons | 60 s | | | | |
| 7 cleanup/signals | 60 s | | | | |
| 8 regression m10-linear-v1 | 60 s | | | | |

Record everything (date, machine, device node, profile, duration, each
checklist item, observations, deviations):

```text
date / machine / device node: ______________________________________________
M11 observations: __________________________________________________________
deviations: ________________________________________________________________
```

## 6. Stop / fail-open criteria

The takeover is **bounded and fail-open** (M10_TASK.md; unchanged for M11):

- Any stop — deadline, SIGINT/SIGTERM, countdown cancel, bridge/output
  fault, stream error, or panic fallback — converges on the one ordered
  cleanup: output release → recorder finalize → ungrab → close. The kernel
  also releases the grab when the fd closes at process exit, so a SIGKILL or
  crash leaves the touchpad usable, but with no ordered cleanup guarantee.
- **Stop the procedure immediately and record** any of the following as a
  FAIL — do not proceed to a later stage:
  - the touchpad does not return to normal operation after a run (residual
    grab, stuck button/scroll, pointer unusable);
  - an exit code outside the documented set, a panic, or a cleanup failure
    that leaves the device/output unreleased;
  - a pointer jump/teleport, non-finite or wildly unbounded emission, or an
    output/arbiter fault during a run;
  - M11 pointer behavior that materially deviates from the calibrated
    expectation (dead zone swallowing sustained motion, uncontrollable
    gain, drift, or re-acceleration of the M6-calibrated deltas);
  - any Stage 1–8 checklist item failing.
- A cancelled/refused portal authorization (exit 3, no panic) is a
  documented outcome, not itself a failure of the profile: record it and
  retry the stage; the system pointer must remain usable.
- **Fail-open:** a failed M11 stage leaves M11 **live-unqualified**; the
  `m10-linear-v1` profile and the recorded M10 acceptance are unaffected,
  and `--output-qualified` remains an operator attestation, not measurement
  evidence.

## 7. Explicit non-claims and qualification scope

- M11 is **experimental, provisional, opt-in only, and never the default**
  (M11_TASK.md).
- M11 makes **no macOS equivalence claim**.
- **Code-complete is not live-qualified** (M11_TASK.md §3, §14): passing all
  offline gates and this review says nothing about real-desktop behavior.
- M10 acceptance does not qualify M11, and passing this M11 acceptance does
  not qualify or imply anything about **M12** — M12 work has not begun and
  no M12 milestone is implicated by M11 in any state.
- The backend remains `experimental/unqualified` until the user records
  Sections 2–5 and every stage passes; this document was **not** executed
  during implementation or review and its results are empty.

## 8. What the code claims vs. what the user must verify

- The code claims (proven offline/fake-backed): the pure fidelity stage
  (signed radial dead zone, monotonic time-domain velocity, bounded
  smoothstep gain, tracking multiplier), atomic fidelity state in the
  Arbiter frame draft with rollback on a rejected frame, the existing single
  per-axis subpixel remainder, `M11Profile` inheriting every M10 value and
  only adding fidelity, the exact `{m10-linear-v1, m11-fidelity-v1}` CLI
  accepted set with the M11 experimental banner before any side effect, and
  the unchanged bounded takeover contract.
- The user must verify (this document, on a real desktop): low-speed
  precision/dead-zone behavior, normal pointer movement, fast-movement gain,
  reversals/diagonals, long idle/re-anchor, click/tap/tap-drag/drag-lock/
  two-finger/button behavior, cleanup/signals, and regression against
  `m10-linear-v1` — and record Sections 2–5. Until then M11 remains
  **live-unqualified**.

## 9. Exit codes (takeover; documented in the CLI help)

Same documented set as M10 (`docs/M10_ACCEPTANCE.md` §7): 0 clean stop with
all cleanup succeeded; 1 usage; 2 no device/bus/portal; 3 permission /
authorization cancelled; 4 not a candidate / capability missing; 5 output
transport or server-side interruption; 6 device/stream/semantic-output/
device-release failure; 7 recorder or output-release failure; 8 aborted
before the grab (countdown cancel); 9 unexpected/internal (including
status-output failure). Cleanup-failure precedence is deterministic:
recorder finalization (7) > output release (7) > device release (6) >
status-output failure (9) > primary stop reason. SIGKILL/crash cannot run
userspace cleanup — the kernel releases the grab when the fd closes, but no
ordered sequence is guaranteed; never claim it.
