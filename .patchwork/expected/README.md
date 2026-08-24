# Touchpad Runtime — Phase 2 (M7): Offline Interaction Arbiter

A cross-desktop userspace touchpad input system (design.md). The approved
M1–M6 foundation delivers the Phase 1 command-line vertical slice and the
qualified KDE Wayland output backend; M7 adds the **offline, platform-
independent interaction layer** (one arbiter owning the interaction
lifecycle, one-finger linear pointer, and physical left-button lifecycle),
driven only by synthetic/trace-derived frames:

```text
Linux raw evdev event
        ↓
versioned raw trace ───────→ offline replay (same Type-B decoder)
        ↓                            ↓
Type-B MT decoder ───────────→ normalized ContactFrame
        ↓
touchpadctl record / replay / devices / inspect
```

M6 (implemented, pending external review) adds the **desktop output
adapter**: the typed `OutputSink` contract (relative pointer motion,
primary/secondary buttons, pixel-precise smooth scroll) translated onto the
XDG **RemoteDesktop portal** (D-Bus) + **libei** sender stack of the current
KDE Wayland session, with a non-emitting `touchpadctl output-probe` tool and
an explicit, bounded `--emit` path for reviewer qualification:

```text
OutputEvent (typed contract)
        ↓
PortalOutputSink — session lifecycle + held-state tracking + release_all
        ↓
RemoteDesktop portal (zbus, v2) ── EIS fd ──▶ libei sender ──▶ compositor
```

M7 adds the offline decision layer between the decoder and any output:

```text
ContactFrame (synthetic or trace-replay)
        ↓
Interaction Arbiter — Candidate/Committed/Cancelled/Finished
        ↓                 one-finger linear pointer (mm → logical px)
        ↓                 physical left-button lifecycle + drag
FrameDecision (ordered OutputEvents + lifecycle transitions)
        ↓
ArbiterSink — feeds a typed OutputSink (M10 wires the real backend)
```

Status of M6: **approved** (M6_REVIEW.md re-review 5) — the real bounded
`--emit` protocol path passed; the backend remains **`experimental/
unqualified`** for takeover until the user records the remaining A/B
measurements (see `docs/M6_ACCEPTANCE.md`). Status of M7/M8/M9: **approved**
(M7_REVIEW.md re-review 2; M8_REVIEW.md re-review 2; M9_REVIEW.md
Re-review 2). Status of M10: **code approved (M10_REVIEW.md Re-review 1),
live-unqualified / pending user acceptance** — the
static/fake-backed gates pass, but the M6 output calibration and the ordered
10-second, 60-second, then 300-second takeover sequence must be recorded by
the user before any live qualification (`docs/M10_ACCEPTANCE.md`;
`--output-qualified` is an operator attestation, not measurement evidence).
Status of M11: **implemented, under independent re-review (not yet
review-approved); experimental, opt-in `m11-fidelity-v1`, never the default,
no macOS equivalence claim; live-unqualified** — M11 code completion does not
imply live qualification, and M11 stays live-unqualified until the separate,
later M11-specific user acceptance (`docs/M11_ACCEPTANCE.md`, written, not
executed) is passed. M10 acceptance does not qualify M11. M12 has not begun.
M1–M6 are approved.

---

## Workspace

| Crate | Role |
| --- | --- |
| `crates/touchpad-core` | Platform-agnostic types and contracts (M1, approved); **M7–M9: Interaction Arbiter + one-finger linear pointer + physical left-button lifecycle + tap/tap-and-drag/drag-lock + two-finger 2D scroll + secondary tap + buttonpad click (offline, approved)**; **M10: `m10-linear-v1` takeover profile**; **M11: experimental `m11-fidelity-v1` one-finger pointer fidelity (opt-in, never default, no macOS equivalence claim, live-unqualified)** |
| `crates/touchpad-trace` | Versioned JSON-Lines raw trace + replay boundary (M2, approved) |
| `crates/touchpad-linux` | Linux device boundary, Type-B decoder, grab, runtime, recorder, signals (M3/M4/M5 approved); **M10: `TakeoverBridge` + deferred-cleanup step + `Sys::poll` bounded-readiness seam** |
| `crates/touchpad-desktop` | KDE Wayland output adapter: RemoteDesktop portal (zbus) + runtime-loaded libei sender transport, session lifecycle, emit pattern, environment probe (M6, approved; backend `experimental/unqualified`); **M10: reusable prepared `StreamingOutput` session + factory** |
| `apps/touchpadctl` | CLI: `devices`, `inspect`, `record`, `replay` (M5 approved); `output-probe [--emit]` (M6, approved); **`takeover` (M10 code approved, live-unqualified; M11 adds the accepted `--profile` value `m11-fidelity-v1` — experimental, live-unqualified)** |

## M7: offline interaction layer (implemented; R1–R2 repaired, pending re-review)

The M7 arbiter lives in `touchpad-core` and is **purely offline and
platform-independent**: it consumes normalized `ContactFrame`s (synthetic or
trace-replay — both paths are proven to produce identical decisions) and
returns typed `FrameDecision`s. It never instantiates a real output sink;
`ArbiterSink` feeds decisions to any typed `OutputSink` (M10 wires the real
backend, which remains gated).

- **One arbiter owns competition** — `Candidate / Committed / Cancelled /
  Finished` lifecycle with a pure, exhaustively-tested transition validator;
  at most one one-finger interaction is committed at a time; a second live
  contact, discontinuity, missing required coordinates, timestamp or
  sequence regression deterministically cancel it with no further movement.
- **Model validation gates every frame** — `Arbiter::frame` consumes
  `ContactFrame::validate()`: any `Error`/`Fatal` diagnostic (negative live
  tracking id, non-finite/out-of-range pressure, non-finite orientation,
  negative ellipse axis, duplicate slot) rejects the whole frame atomically
  with structured `InvalidFrame { codes, reason }` and zero state/button/
  baseline change, even when the frame also carries a physical-button edge.
  Warning-only cases (incomplete `Began` contact) keep their policy: no
  candidate/output and a diagnostic.
- **Candidate period emits nothing** — no `PointerMove` and no synthetic
  button before the configured motion threshold; the first committed
  movement accounts exactly once for the displacement accumulated since the
  candidate anchor.
- **Explicit linear mapping** — `ArbiterConfig::new(threshold_mm,
  LogicalPixelsPerMm::try_new(px_per_mm))`; both validated (positive/finite).
  Positions stay `Millimeters`; output stays `LogicalPixels`; per-axis
  sub-pixel remainders are carried exactly (`Σ emitted + remainder ==
  Σ scaled`) and reset on cancellation/finish/release so residue never
  leaks between contacts.
- **Physical left-button lifecycle** — exactly one down on `false→true`,
  one up on `true→false`, stable state silent; repeated pairs pass through
  (physical double-click); press precedes drag movement, final movement
  precedes release; release is never suppressed by cancellation, added
  fingers, missing coordinates, or discontinuity; idempotent `release_all`
  is the M10 shutdown path.
- **Delivery-aware, fail-stop `ArbiterSink`** — events are submitted with
  explicit acknowledgement: a rejected `ButtonDown` is never treated as
  delivered (no unmatched up); any partial submission faults the adapter and
  blocks further frames until cleanup; cleanup releases only state the sink
  accepted, invokes the wrapped sink's own `release_all`, stays retryable
  after either release or cleanup failure, and resets the arbiter only at
  the acknowledgement boundary. Errors preserve the failed event/index,
  accepted prefix, primary failure, and cleanup failure when both exist.
- M7 does **not** implement tap, tap-and-drag, drag lock, scroll,
  right/middle mapping, Force Click, pressure, or acceleration curves
  (M8/M9/M11+).

## M11: experimental one-finger pointer fidelity (implemented, under review)

M11 layers an **experimental, opt-in, never-default** one-finger
pointer-fidelity stage on the approved M7–M9 interaction policy, exposed only
as the `m11-fidelity-v1` value of the existing mandatory `--profile` option
(the accepted set is exactly `{m10-linear-v1, m11-fidelity-v1}`). It makes
**no macOS equivalence claim**. The pure, platform-independent stage adds a
signed radial dead zone, a monotonic time-domain velocity estimate, a bounded
smoothstep gain curve, and an explicit tracking multiplier for committed
one-finger millimeter motion; `M11Profile` inherits every M7–M9 value from
`m10-linear-v1` without copying constants. Fidelity state lives in the
Arbiter's atomic draft (a rejected frame rolls it back), the fidelity-disabled
M10 path is unchanged, and the CLI prints the experimental banner before any
device/output/recorder/countdown/grab side effect. Status: **implemented,
under independent re-review (not yet review-approved), live-unqualified** —
code completion does not imply live qualification; M11 stays live-unqualified
until the separate, later user acceptance in `docs/M11_ACCEPTANCE.md`
(written, not executed) is passed. M10 acceptance does not qualify M11. M12
has not begun.

## Build and test

Stable Rust. The workspace declares `rust-version = 1.87` as its **MSRV —
the real minimum of the locked dependency graph** (M6 re-review R6: zbus
5.19 and the zvariant family declare `rust-version 1.87`, so the earlier
1.85 claim was never the real minimum). The declared MSRV has **not been
independently tested yet**; the gates in this milestone ran on rustc/cargo
**1.97.1**. No root, no desktop
session, no session bus, no `/dev/input`, no libei, and no portal are
needed for the automated tests: every test drives the mockable `Sys` seam
and the fake portal/transport seams, and the few real OS surfaces exercised
are side-effect-free (`sigaction`, `raise`, `read_dir`/`open` on
nonexistent paths, dlopen of `libei.so.1` as a probe, a session-bus
reachability probe). No test opens or grabs a device and no test emits real
desktop input:

```text
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the CLI:

```text
cargo run -p touchpadctl -- --help
```

## The commands

### `touchpadctl devices`

Enumerate `/dev/input/event*` and explain how each node was judged
(candidate / rejected / inaccessible, with the evidence and reasons):

```text
$ touchpadctl devices
input event nodes in /dev/input: 1
[1] /dev/input/event1 — "Synaptics Touchpad" — candidate
      candidate: Type-B multitouch pointer device, slot_count=10, physical_buttons=true
      evidence: path /dev/input/event1 matches the /dev/input/event* enumeration pattern
      evidence: reports EV_KEY
      ...
candidate touchpad: /dev/input/event1 (Synaptics Touchpad)
```

Exit codes: `0` at least one candidate; `2` no `/dev/input`; `3` permission
denied on `/dev/input`; `4` enumeration succeeded but no touchpad candidate
(a clear message explains what to check).

### `touchpadctl inspect DEVICE`

Probe one device node and show identity, capabilities, axes (min/max/fuzz/
flat/resolution), slot count, and the verdict with reasons. A non-candidate
device is printed in full and exits `4` with the rejection reasons; a
missing node exits `2`; a permission problem exits `3`.

### `touchpadctl record DEVICE OUTPUT [--grab]`

Record raw evdev events into a versioned JSON-Lines trace. The raw events
are written **before** they are decoded, so a decoder bug can never lose the
raw input needed to reproduce it:

```text
$ touchpadctl record /dev/input/event1 trace.jsonl
recording /dev/input/event1 -> trace.jsonl (grab: false); stop with Ctrl-C (SIGINT) or SIGTERM
^C
recording stopped: SIGINT/SIGTERM (controlled stop)
trace: trace.jsonl — 1284 raw events recorded, 413 frames decoded (…)
cleanup: recorder ok, ungrab ok, close ok
```

`--grab` is **off by default**, is an explicit opt-in, and is **record-only**
(it is rejected with a usage error — exit 1 — for every other command, and a
repeated `--grab` is rejected too).

### `touchpadctl replay INPUT`

Offline replay of a raw trace through the **exact same Type-B decoder** used
by live input (there is no second decoder). One JSON `ContactFrame` per line
on stdout, a summary on stderr. `replay` is **purely offline**: it opens the
trace file only, never touches `/dev/input`, and runs as an ordinary user in
CI or headless environments.

### `touchpadctl output-probe [--emit]` (M6)

Probe the KDE Wayland output backend (XDG RemoteDesktop portal + libei).
**The default is a non-emitting dry-run** — it reports the environment, the
capabilities that would be negotiated, and the exact steps `--emit` would
run; it never moves the pointer, clicks, scrolls, or touches `/dev/input`:

```text
$ touchpadctl output-probe
backend state: experimental/unqualified
platform: linux (WAYLAND_DISPLAY=wayland-0, XDG_SESSION_TYPE=wayland, XDG_CURRENT_DESKTOP=KDE)
session bus: reachable
RemoteDesktop portal: available (interface version 2, device types 7 [pointer available])
libei: libei.so.1 loadable
requested capabilities (negotiated only by an actual --emit): relative pointer, primary button, secondary button, pixel-precise smooth scroll
--emit would:
  1. connect to the D-Bus session bus
  ...
  8. release all held state, disconnect, close the session
note: the backend stays experimental/unqualified until a reviewer runs and measures --emit
```

**`--emit` is an explicit opt-in for real desktop emission** (rejected for
every other command; duplicates rejected). It prints a visible warning, a
3-second countdown (Ctrl-C cancels with exit 8), then emits a **short,
fixed, bounded test pattern**: relative pointer moves of **+10 / +50 / +200
px**, a primary click, a pixel-precise smooth scroll (begin, −120 px, −240
px, end), and a secondary click — each step gated on the negotiated
capability (missing capabilities are skipped and reported, never faked). On
every path (success, partial send failure, disconnect, cancellation) the
adapter releases all held button/scroll state, disconnects, and closes the
session, and the failure is an honest structured result. `--emit` is left to
the reviewer: **do not run it casually — it moves the real pointer and
clicks on the real desktop.**

## Ordinary-user offline replay

No special permissions are needed to replay a trace:

```text
$ touchpadctl replay single_contact.jsonl            # works as any user
```

Known offline limitation, reported honestly: a trace containing `SYN_DROPPED`
cannot be replayed offline, because resynchronization requires a kernel
snapshot (`EVIOCGMTSLOTS`/`EVIOCGKEY`) that offline replay has no source
for — the command fails with a structured error (exit 6).

## Device permission diagnostics

Live commands (`devices`, `inspect`, `record`) read `/dev/input/event*`.
On most systems those nodes are `660 root:input`, so your user must be in
the `input` group (or root). Every permission problem is reported as an
actionable diagnostic with a stable exit code, never a panic.

## `--grab` risk

`--grab` issues `EVIOCGRAB(1)`: while held, this process **exclusively
owns** the touchpad. The desktop and other applications will **not**
receive its events, so the system pointer/tap/gesture behavior is unusable
until recording stops. The device is released by `EVIOCGRAB(0)` on every
controlled path, and the kernel also releases the grab when the fd closes at
process exit — but only an orderly run can guarantee the ordered
recorder-finalization→ungrab→close sequence.

## Controlled signal exit and non-guaranteed failures

`record` installs a `SIGINT`/`SIGTERM` handler (without `SA_RESTART`, so the
blocking read is interrupted) that records a stop request in a
**process-lifetime static** — the async handler dereferences no caller-owned
memory, so no guard-teardown interleaving can ever leave it touching freed
storage (M5 re-review R1). Every exit path — normal exit, `SIGINT`/`SIGTERM`,
EOF/device unplug, decoder error, recorder error — runs the same ordered,
idempotent cleanup, performed entirely by the runtime so the fallible
recorder `finish` can never be postponed past the device release:

1. stop accepting new work;
2. end the semantic-output lifecycle (Phase 1 has no real output backend);
3. recorder finalization — `finish` (which flushes) plus best-effort
   recorder destruction — before the device release, never after;
4. ungrab at most once (`EVIOCGRAB(0)`, never retried even on failure);
5. close the fd even if the ungrab failed (fail-open: the kernel releases
   the grab on close);
6. print the structured status and exit with a stable code.

The M6 output adapter follows the same fail-open discipline on the desktop
side: `release_all` is idempotent and runs on normal shutdown, fatal
shutdown, partial send failure, and fallback `Drop`, so no path leaves a
logically held button or an open scroll lifecycle; the transport disconnect
is the compositor-side backstop that resets any remaining emulated state.

**Not guaranteed:** `SIGKILL`, a kernel crash, or a hard power loss cannot
run userspace cleanup. The kernel automatically releases an evdev grab when
the owning fd is closed by process exit; the portal closes a remote-desktop
session when the client exits; but the ordered userspace sequences above are
only guaranteed on paths this process can run.

## M6 ABI choices (documented, environment-based)

- **Portal**: `org.freedesktop.portal.RemoteDesktop` **interface version 2**
  (observed on this host by D-Bus introspection) with `ConnectToEIS`
  returning the EIS socket fd; `SelectDevices` requests the pointer device
  type. The zbus client subscribes to the portal `Response` signal **before**
  each method call (handle-token request paths) so responses cannot race the
  subscription. Tokens are generated from the D-Bus object-path-safe
  alphabet (`[A-Za-z0-9_]`) and every predicted request path is validated
  with zvariant before the match rule is registered; `CreateSession` carries
  distinct `handle_token` and `session_handle_token` (M6 re-review R12).
  The `CreateSession` response's `session_handle` is decoded according to
  its wire ABI — D-Bus **string** (`s`) whose contents are the session
  object path (the portal XML notes the object path was "erroneously
  implemented as `s`" and stays `s` for compatibility), not `o` (M6
  re-review R13).
- **libei**: soname `libei.so.1` (1.6.0 installed), **loaded at run time**
  via `libloading` — a missing library is a structured
  `LibraryMissing` result (exit 4), never a build failure, so the workspace
  builds and tests without the library. The FFI is a minimal hand-written
  boundary with documented safety invariants; every other module is
  `#![forbid(unsafe_code)]`, and libei objects are handled only through
  opaque handles that safe code cannot fabricate. Each handle owns its own
  `Arc` to the loaded library, so the library cannot be unloaded while any
  handle exists (M6 re-review R7).
- **D-Bus**: pure-Rust `zbus` blocking API — no system D-Bus library is
  linked, so offline builds/tests work without a session bus.

## Live platform limitation (x86_64 Linux only)

The live Linux input path (`touchpad-linux`'s `sys::ffi` and the
`struct input_event` decoder) is implemented and verified **only for x86_64
Linux** (24-byte `input_event`, two 8-byte `timeval` fields). Other Linux
ABIs fail at compile time rather than misdecode (M4 review RR3). The libei
native transport is also Linux-only (`cfg(target_os = "linux")`); on other
platforms `output-probe` honestly reports the unsupported platform. Offline
`replay`, the mock seams, and every automated test are portable.

## Automated tests vs. environment probing vs. real-machine verification

These three are strictly separated — nothing below is a claim of real
hardware or real desktop validation:

1. **Automated tests** (`cargo test --workspace`, **872 tests**): run on any
   machine without `/dev/input`, root, a desktop session, a session bus, or
   libei. They use the mockable `Sys` seam, the hand-written trace fixtures,
   and the fake portal/transport seams; the native libei transport's
   event-loop algorithm is tested below raw FFI over a scripted seam (no
   real library, fd, or emission — M6 re-review R8). **No test opens or
   grabs a real device and no test emits real desktop input** (the only real
   OS surfaces exercised are the side-effect-free Linux FFI tests:
   `sigaction`, `raise(SIGINT)`, `read_dir`/`open` on nonexistent paths, a
   dlopen probe of `libei.so.1`, and a session-bus reachability probe).
2. **Environment probing (this session, read-only)**: the KDE Wayland
   session is present (`WAYLAND_DISPLAY=wayland-0`,
   `XDG_CURRENT_DESKTOP=KDE`), the session bus is reachable, the
   RemoteDesktop portal reports interface version 2 with the pointer device
   type, and `libei.so.1` is loadable. `touchpadctl output-probe` (dry-run)
   reports all of this without creating a session or emitting input. These
   observations are environment facts, not validation.
3. **Real-machine/desktop verification (not performed, explicitly reserved
   for the reviewer)**: the `--emit` measurement (relative-delta A/B
   displacement, pixel scroll, button release, cancel/refusal cleanup) and
   the hardware items from M5 (real grab exclusivity, real `SYN_DROPPED`
   resync, unplug). The backend stays `experimental/unqualified` until the
   reviewer completes `docs/M6_ACCEPTANCE.md` §3.

## Exit codes (stable CLI contract)

| Code | Meaning |
| --- | --- |
| 0 | success; **takeover: the session ended (deadline reached, or SIGINT/SIGTERM during the loop) with ALL required cleanup succeeding — the stderr status line states the exact stop reason** |
| 1 | usage / argument error |
| 2 | input directory or device node not found (no `/dev/input`); **output-probe: no D-Bus session bus or no RemoteDesktop portal; takeover: device node missing / no session bus / no portal** |
| 3 | permission denied reading the input directory or device node; **output-probe: authorization cancelled or refused by the user/portal; takeover: permission denied / authorization cancelled or refused** |
| 4 | no touchpad candidate (or the inspected device is not a candidate); **output-probe: libei library missing, portal protocol too old, or a required capability missing; takeover: same, refused before the recorder/grab** |
| 5 | trace file error (missing, corrupt, schema mismatch, time regression); **output-probe: transport disconnected or the session timed out; takeover: output transport disconnected/timed out during preparation, or a server-side interruption (device pause/removal, seat removal, disconnect)** |
| 6 | device stream error (EOF/unplug, torn read, decoder failure) or a device-release failure (ungrab/close failed during cleanup); **output-probe: a send failed (partial send failure); takeover: a device stream error, a semantic-output fault, or a device-release failure** |
| 7 | recorder error (trace output could not be written or finalized); **output-probe: releasing held button/key/scroll state failed; takeover: recorder output/finalize failure or an output-release failure** |
| 8 | stopped by SIGINT/SIGTERM (controlled stop; trace flushed, device released — only when the finalization actually succeeded; otherwise 6 or 7 with the full diagnostic); **output-probe: aborted by the user before/during emission; takeover: aborted by the user before the takeover began (countdown cancel / signal during countdown) — nothing was grabbed, the prepared output session was released, the recorder finalized, the device closed** |
| 9 | unexpected/internal error (including status-output failure) |

The `output-probe` dry-run always exits 0 when it completes (a completed
probe is a successful probe; the findings are the report) — scripts should
read the report rather than the dry-run exit code.

## Third-party dependencies

See `THIRD_PARTY.md`. M6 added two pure-Rust crates — `zbus` (portal D-Bus
client) and `libloading` (run-time libei loading) — and uses the system's
`libei.so.1` at run time; nothing links libei at build time.

## Acceptance documentation

See `docs/M5_ACCEPTANCE.md` (M5), `docs/M6_ACCEPTANCE.md` (M6, including
the reviewer-run `--emit` measurement procedure that decides qualification),
`docs/M10_ACCEPTANCE.md` (M10, the user-run 10/60/300-second takeover
sequence and the M6 output-calibration table that must be filled before
honestly passing `--output-qualified`), and `docs/M11_ACCEPTANCE.md` (M11,
the future, **not-yet-executed** user-run acceptance for the experimental
`m11-fidelity-v1` profile; M10 acceptance does not qualify M11).
