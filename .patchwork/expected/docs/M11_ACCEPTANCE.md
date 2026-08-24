# M11 Acceptance — Experimental One-Finger Pointer Fidelity (`m11-fidelity-v1`)

Status: **FUTURE USER-RUN PROCEDURE — WRITTEN, NOT EXECUTED**. M11 is
implemented and its `M11_REVIEW.md` blocking findings (R1–R4) are repaired,
but the independent re-review is **not yet passed**: M11 is **not
review-approved, not code-complete, and not live-qualified**. This document
is the **separate, later M11-specific user acceptance** required by
`M11_TASK.md` §3. It must **not** be executed by the implementation or the
review process, and M11 stays **live-unqualified** until the user performs
the sequence below. **M10 acceptance does not qualify M11.**

This document strictly separates:

1. **M11 code-complete** — every offline gate passes and independent code
   review approves the M11 implementation (`M11_TASK.md` §3/§14). As of this
   writing this is **not yet true** (re-review pending); code completion does
   not confer live qualification.
2. **M10/output live qualification** — the mandatory **prerequisite** for any
   M11 live run: M10 code approved, the M6 output-calibration evidence
   recorded, and the ordered 10-second, 60-second, then 300-second M10
   acceptance with `m10-linear-v1` passed (`docs/M10_ACCEPTANCE.md`).
3. **M11 live qualification** — this document's future user-run 10/60/300-
   second sequence with `m11-fidelity-v1`. Until it passes, M11 remains
   `live-unqualified` and `--output-qualified` stays an operator attestation
   rather than measurement evidence.

## 1. Preconditions (every box must hold before any M11 live run)

- [ ] M10 is code-approved and its static/fake-backed gates pass.
- [ ] The user recorded the M6 output-calibration table
      (`docs/M6_ACCEPTANCE.md` §3) and passed the ordered 10/60/300-second M10
      acceptance with `m10-linear-v1` (`docs/M10_ACCEPTANCE.md`).
- [ ] M11 is code-complete: `cargo fmt --all -- --check`, `cargo clippy
      --workspace --all-targets --locked -- -D warnings`, `cargo test
      --workspace --locked`, and `cargo test --release --workspace --locked`
      all pass, and independent review approves the M11 implementation.
- [ ] The experimental M11 banner was read and understood: `m11-fidelity-v1`
      is EXPERIMENTAL, UNCALIBRATED, NOT the default, makes NO macOS-
      equivalence claim, and NO live M11 validation has occurred.

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

The M11 takeover command (do not run yet — see Section 3):

```text
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m11-fidelity-v1 \
  --max-duration-seconds N
```

Every opt-in is mandatory and independently validated; `N` is an integer in
`1..=300`. No zero, overflow, missing, repeated, or unlimited form is
accepted. The command is foreground-only: no daemon, no fork/background mode,
no autostart, no service file, no persistence, no config mutation, and no
system-setting write. The experimental banner is printed **before** any
device/output/recorder/countdown/grab side effect.

## 2. What M11 code-complete covers vs. what the user must verify

- The code claims (proven by the fake-backed automated tests): a pure,
  platform-independent fidelity stage for committed one-finger millimeter
  motion — a signed radial dead-zone (`0.09 mm`), a monotonic time-domain
  velocity estimate (EMA, `20 ms` tau), a continuous bounded smoothstep gain
  curve (`50–600 mm/s` → gain `1.0–2.0`), an explicit tracking multiplier
  (`1.0`), and the existing per-axis subpixel remainder; `M11Profile`
  inherits every M7–M9 value from `m10-linear-v1` without copying constants;
  fidelity state lives in the Arbiter's atomic draft and rolls back with a
  rejected frame; the fidelity-disabled M10 path is unchanged; the CLI accepts
  exactly `{m10-linear-v1, m11-fidelity-v1}` and prints the experimental
  banner before any side effect; direct synthetic frames and replay-derived
  frames produce identical M11 decisions.
- The user must verify (this document): the real desktop behavior with
  `m11-fidelity-v1` — dead-zone feel, gain at slow vs. fast movement,
  reversal/diagonal behavior, long-gap re-anchor behavior,
  tap/tap-and-drag/drag-lock/two-finger scroll still working, and the bounded
  10/60/300-second sequence below. Until then, M11 remains
  **experimental/unqualified**.

## 3. M11 live acceptance — exact 10 / 60 / 300-second sequence (future)

Run only after every Section 1 precondition holds. Keep an external keyboard
and mouse connected — the touchpad is grabbed exclusively — and keep a second
terminal ready: `kill -TERM <pid>` (with `<pid>` = the takeover process id
printed by the command) as the independent escape route. Ctrl-C in the
takeover terminal is the primary route; the configured maximum duration is the
automatic backstop.

### 3.1 Run 1 — 10 seconds

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-10s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 10
```

Checklist (pass/fail each):

- [ ] Portal authorization dialog appears; approve it once.
- [ ] The touchpad is grabbed: the system pointer stops responding to the
      touchpad (the external mouse still works).
- [ ] The M11 banner (experimental / uncalibrated / not the default / no
      macOS equivalence / no live M11 validation) was printed before any
      side effect.
- [ ] Small jittering one-finger motion produces no pointer movement (dead
      zone); movement resumes when it exceeds the radius.
- [ ] Slow, consistent motion is briefly delayed until the dead-zone radius
      is reached, then moves smoothly at roughly the M10 scale (min gain).
- [ ] Faster motion smoothly raises pointer speed (gain curve) with no jumps,
      and the pointer stops immediately when the finger stops (no momentum).
- [ ] Reversal and diagonal motion behave (signed radial cancellation; both
      axes scaled isotropically).
- [ ] A long pause (≥ 150 ms) between movements re-anchors without a pointer
      jump; a fresh interaction starts with no stale velocity.
- [ ] Physical primary click and click-drag work; tap-to-click, double tap,
      tap-and-drag, and drag lock behave as in M10.
- [ ] Two-finger natural scroll and two-finger secondary click behave as in
      M10.
- [ ] After the deadline (10 s) the process exits 0 (or the documented
      cleanup-failure code) and prints the cleanup status.
- [ ] After the run, the physical touchpad works normally again (no residual
      grab, no stuck button/scroll); the trace `/tmp/m11-10s.jsonl` exists
      and replays: `touchpadctl replay /tmp/m11-10s.jsonl`.
- [ ] The portal session closed (the KDE authorization indicator cleared).

Record results in the Section 4 table. **Only after Run 1 passes** continue
to Run 2.

### 3.2 Run 2 — 60 seconds

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-60s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 60
```

Same checklist as Run 1, plus:

- [ ] During the run, send `kill -TERM <pid>` from the second terminal
      (independent escape route): the process exits cleanly (0 when all
      cleanup succeeded) and the touchpad is restored.
- [ ] Also test Ctrl-C in the takeover terminal on another run: same clean
      result.
- [ ] No duplicate or missing events (no double clicks, no stuck scroll).

### 3.3 Run 3 — 300 seconds (5 minutes)

```text
touchpadctl takeover /dev/input/eventN /tmp/m11-300s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m11-fidelity-v1 --max-duration-seconds 300
```

Same checklist as Run 1/2. The 5-minute run validates the bounded loop under
prolonged idle and continuous use with the fidelity stage active.

## 4. Result table

| Run | Pass/Fail | Dead-zone feel | Gain vs. speed | Duplicate/missing events | Stuck button/scroll | Cleanup messages | Trace path | Deviations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10 s | | | | | | | | |
| 60 s | | | | | | | | |
| 300 s | | | | | | | | |
| signal stop | | | | | | | | |

Observations (record anything notable):

```text
________________________________________________________________________
```

## 5. Explicit non-claims

M11 has **no momentum, no palm/thumb classification, no pinch/rotate/swipes,
no Force Touch, no pressure, and no haptics**. `m11-fidelity-v1` is an
**experimental, opt-in, uncalibrated** profile — not the default, and **not a
macOS-equivalence claim**. **No live M11 validation has occurred.** M10
acceptance does not qualify M11, and `--output-qualified` is an operator
attestation, not measurement evidence. The takeover remains **foreground-only
and bounded** (1–300 seconds); there is no daemon, no background mode, no
autostart, and no service.

## 6. Exit codes (takeover; the same bounded contract as M10 — only the
   `--profile` value differs)

Exit codes are identical to the M10 takeover contract (`docs/M10_ACCEPTANCE.md`
§7 and the CLI help): `0` session ended (deadline reached, or
SIGINT/SIGTERM during the loop) with ALL required cleanup succeeding; `1`
usage / argument error; `2` device node missing / no session bus / no portal;
`3` permission denied / authorization cancelled or refused; `4` device not a
candidate / libei missing / protocol too old / required capability missing
(refused before the recorder/grab); `5` output transport disconnected or
timed out during preparation, or a server-side interruption; `6` device
stream error / semantic-output fault / device-release failure; `7` recorder
output/finalize failure or an output-release failure; `8` aborted by the user
before the takeover began (nothing was grabbed, the prepared output session
was released, the recorder finalized, the device closed); `9` unexpected /
internal error (including status-output failure).

## 7. What must change before this procedure may run

- The M11 independent re-review must pass and the workspace must record M11
  as code-complete (offline gates + review approval), and
- the user must have satisfied the Section 1 M10/M6 prerequisites.

Until then, do **not** run this procedure, do **not** run `touchpadctl
takeover` with `--profile m11-fidelity-v1` on a machine you rely on, and do
not begin M12.
