# M10 Acceptance — Bounded, Fail-Open Live Takeover Slice

Status: **code approved by `M10_REVIEW.md` Re-review 1; live acceptance NOT
YET PERFORMED**. The static/fake-backed gates and independent review pass;
the steps below are the remaining user-run qualification gate.
M10 remains **live-unqualified / pending user acceptance** until the user
completes the 10-second, 60-second, then 300-second sequence below and records
the results. Do **not** run `touchpadctl takeover` on a machine you rely on
until you have read this document and completed the M6 output calibration
(Section 2).

This document strictly separates:

1. **Automated tests** — run everywhere, no portal/display/session
   bus/libei/hardware/root needed (all M10 tests are fake-backed: no test
   opens `/dev/input`, grabs a real fd, creates a real portal/libei session,
   emits desktop input, sleeps, or modifies system settings).
2. **M6 output calibration** — the mandatory measurement gate that must be
   recorded by the user **before** honestly passing `--output-qualified`.
3. **Live acceptance** — the user-run 10/60/300-second takeover sequence.

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

The takeover command (do not run yet — see Section 3):

```text
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m10-linear-v1 \
  --max-duration-seconds N
```

Every opt-in is mandatory and independently validated; `N` is an integer in
`1..=300`. No zero, overflow, missing, repeated, or unlimited form is
accepted. The command is foreground-only: no daemon, no fork/background mode,
no autostart, no service file, no persistence, no config mutation, and no
system-setting write.

## 2. M6 output-calibration table (must be filled by the user BEFORE `--output-qualified`)

The backend is `experimental/unqualified` until the reviewer (you) records
`docs/M6_ACCEPTANCE.md` §3. `--output-qualified` is the **operator
attestation** that this calibration was performed. **It is not itself
measurement evidence** — the numbers below are the evidence, and M10 is not
live-qualified until they are recorded and the acceptance sequence passes.

Run, for each delta, at least 10 repetitions of the bounded probe and measure
the actual on-screen pointer displacement (screen ruler/grid, before/after):

| Run | Delta (px) | Sample | Displacement (px) | Notes |
| --- | --- | --- | --- | --- |
| 1 | 10 | 1 | | |
| 1 | 10 | 2 | | |
| … | 10 | ≥ 10 | | |
| 2 | 50 | 1 | | |
| … | 50 | ≥ 10 | | |
| 3 | 200 | 1 | | |
| … | 200 | ≥ 10 | | |

Mean and spread per delta:

```text
10 px:  mean ____  spread ____   (≈ 10 → not re-accelerated)
50 px:  mean ____  spread ____   (≈ 50 → not re-accelerated)
200 px: mean ____  spread ____   (≈ 200 → not re-accelerated)
```

Pixel scroll observation (during the probe's scroll step): the scroll must be
**smooth (pixel-precise)** with no discrete wheel-step conversion visible;
record whether the distance matches −120/−240 px and whether any second
compositor-side acceleration is evident:

```text
pixel scroll: ____________________________________________________________
```

Button release observation: after the probe, no button remains logically held
(no drag mode, no stuck menu), `release_all` is idempotent (exit 0), and the
pointer returns to normal operation immediately:

```text
button release: __________________________________________________________
```

Cancel/refusal cleanup: a cancelled authorization returns exit 3, no panic,
system pointer remains usable:

```text
cancel cleanup: __________________________________________________________
```

**Only when this table is complete and the deltas are measured should you
pass `--output-qualified` on a takeover run.** If the deltas are scaled,
non-linear across deltas, or jittery, do NOT pass it — record the deviation
and stop.

## 3. Live acceptance — exact 10 / 60 / 300-second sequence

### 3.0 Before any run

1. Identify the exact touchpad device:
   ```text
   touchpadctl devices
   touchpadctl inspect /dev/input/eventN
   ```
   The target machine's touchpad is `CIRQ1080:00 0488:1054 Touchpad`
   (KDE shows decimal vendor/product 1160/4180); confirm the node before
   taking over.
2. **Keep an external keyboard and mouse connected** — the touchpad is
   grabbed exclusively and its events are NOT delivered to the desktop while
   the takeover runs.
3. Open a **second terminal** and keep it ready:
   ```text
   kill -TERM <pid>
   ```
   (with `<pid>` = the takeover process id printed by the command) as the
   independent escape route. Ctrl-C in the takeover terminal is the primary
   route; the configured maximum duration is the automatic backstop.
4. Permissions: if the device node is not readable, report the required
   group/udev access (usually membership in the `input` group) rather than
   changing the system. This document never suggests `sudo` as a generic
   solution and never embeds credentials.

### 3.1 Run 1 — 10 seconds

```text
touchpadctl takeover /dev/input/eventN /tmp/m10-10s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m10-linear-v1 --max-duration-seconds 10
```

Checklist (pass/fail each):

- [ ] Portal authorization dialog appears; approve it once.
- [ ] The touchpad is grabbed: the system pointer stops responding to the
      touchpad (the external mouse still works).
- [ ] One-finger movement moves the pointer; the observed pointer scaling
      matches the calibrated deltas (Section 2).
- [ ] Physical primary click and click-drag work (press and move with one
      finger).
- [ ] Tap-to-click produces a primary click.
- [ ] Double tap produces two click pairs.
- [ ] Tap-and-drag and drag lock behave (tap, then quickly touch again and
      drag; after lifting, the drag stays locked; a qualifying tap releases).
- [ ] Two-finger vertical/horizontal/diagonal **natural** scroll is smooth
      (pixel-precise), in the natural direction.
- [ ] **Momentum is explicitly absent** (releasing the fingers stops the
      scroll immediately — M10 has no inertia).
- [ ] Two-finger tap produces a secondary (right) click.
- [ ] Buttonpad two-finger physical click (press the pad while two fingers
      are down) produces a secondary click.
- [ ] After the deadline (10 s) the process exits 0 (or the documented
      cleanup-failure code) and prints the cleanup status.
- [ ] After the run, the physical touchpad works normally again (no residual
      grab, no stuck button/scroll).
- [ ] The trace `/tmp/m10-10s.jsonl` exists and replays:
      `touchpadctl replay /tmp/m10-10s.jsonl`.
- [ ] The portal session closed (the KDE authorization indicator cleared).

Record results in the Section 4 table. **Only after Run 1 passes** continue
to Run 2.

### 3.2 Run 2 — 60 seconds

```text
touchpadctl takeover /dev/input/eventN /tmp/m10-60s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m10-linear-v1 --max-duration-seconds 60
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
touchpadctl takeover /dev/input/eventN /tmp/m10-300s.jsonl \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m10-linear-v1 --max-duration-seconds 300
```

Same checklist as Run 1/2. The 5-minute run validates the bounded loop under
prolonged idle and continuous use.

## 4. Result table

| Run | Pass/Fail | Pointer scaling vs §2 | Duplicate/missing events | Stuck button/scroll | Cleanup messages | Trace path | Deviations |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 10 s | | | | | | | |
| 60 s | | | | | | | |
| 300 s | | | | | | | |
| signal stop | | | | | | | |

Observations (record anything notable):

```text
________________________________________________________________________
```

## 5. Explicit non-claims

M10 has **no acceleration, no momentum, no palm/thumb classification, no
pinch/rotate/swipes, no Force Touch, no pressure, and no haptics**. The
`m10-linear-v1` profile is a conservative linear bring-up profile — not a
macOS-equivalence claim and not a production default. The takeover is
**foreground-only and bounded** (1–300 seconds); there is no daemon, no
background mode, no autostart, and no service.

## 6. What the code claims vs. what the user must verify

- The code claims: preparation order (grab is the final step), a truly
  bounded event loop, the fallible frame bridge (first fault stops output),
  and one unified ordered shutdown (output release → recorder finalize →
  ungrab → close) with every cleanup failure preserved — all proven by the
  fake-backed automated tests.
- The user must verify (this document): the real desktop behavior — pointer
  displacement, pixel scroll smoothness, button release, cancel cleanup, and
  the 10/60/300-second sequence — and record the Section 2 calibration table.
  Until then, the backend and M10 remain **experimental/unqualified** and
  `--output-qualified` is an attestation, not data.

## 7. Exit codes (takeover; documented in the CLI help)

| Code | Meaning |
| --- | --- |
| 0 | session ended (deadline reached, or SIGINT/SIGTERM during the loop) with ALL required cleanup succeeding — the stderr status line states the exact stop reason |
| 1 | usage / argument error |
| 2 | device node missing / no session bus / no portal |
| 3 | permission denied / authorization cancelled or refused |
| 4 | device not a candidate / libei missing / protocol too old / required capability missing (refused before recorder/grab) |
| 5 | output transport disconnected or timed out during preparation, or a server-side interruption (device pause/removal, seat removal, disconnect) |
| 6 | device stream error (EOF/unplug, torn read, decoder failure, resync failure) / a semantic-output fault / a device-release failure (ungrab/close failed) |
| 7 | recorder output/finalize failure or an output-release failure |
| 8 | aborted by the user before the takeover began (countdown cancel / signal during countdown) — nothing was grabbed, the prepared output session was released, the recorder finalized, the device closed |
| 9 | unexpected/internal error (including status-output failure) |

Cleanup-failure precedence (deterministic): recorder finalization (7) >
output release (7) > device release (6) > status-output failure (9) > primary
stop reason. The message preserves the primary reason and every cleanup
failure. `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
cleanup: the kernel releases the grab when the fd closes at process exit, but
no ordered sequence is guaranteed — never claim it.
