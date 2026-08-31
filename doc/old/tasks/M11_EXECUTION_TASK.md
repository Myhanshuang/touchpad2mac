# M11 Execution Task — Complete `m11-fidelity-v1`

Status: ACTIVE IMPLEMENTATION TASK.

This file is the implementation checklist for M11. `M11_TASK.md` remains the
highest-authority behavioral contract. Also read:

- `M11_TASK.md` in full;
- `docs/superpowers/specs/2026-08-21-m11-pointer-fidelity-design.md`;
- `docs/superpowers/plans/2026-08-21-m11-pointer-fidelity.md`;
- the current implementation and M7–M10 tests/contracts that M11 extends.

If this execution file conflicts with `M11_TASK.md`, follow `M11_TASK.md`.
In particular, duplicate-timestamp behavior follows `M11_TASK.md` §7.2:
accumulate `P` and `V_pending`, do not update velocity, and **do not evaluate
or flush the dead zone on the duplicate-timestamp frame**.

## 0. DSH execution rule

Do the work. Do not stop after reading files, planning, describing the next
step, or reporting that a partial test passes. Continue editing, compiling,
testing, fixing, and re-testing until every applicable checklist item below is
complete or an external blocker makes further progress impossible.

When a command or test fails, inspect the failure, repair it, and continue.
Preserve already-correct work. Do not revert unrelated M1–M10 behavior.

The final response must summarize actual completed changes and exact gate
results. A message such as “now I will inspect…” is not completion.

## 1. Hard safety boundary

Implementation and validation are offline/fake-backed only.

Do **not**:

- run `touchpadctl takeover` against a real device;
- open/grab a real input device;
- create a real portal/libei session;
- emit desktop input;
- change system/KDE/libinput settings;
- execute `docs/M11_ACCEPTANCE.md`;
- begin M12;
- add daemon/autostart/unbounded takeover behavior.

M10/output remains `live-unqualified` pending its existing M6 calibration and
10/60/300-second M10 user acceptance. M11 remains independently
`live-unqualified` after code completion until a later M11-specific user
acceptance is performed.

## 2. Current workspace state — continue from here

The workspace already contains partial M11 work. Inspect it; do not start over.

Existing partial implementation:

- `crates/touchpad-core/src/fidelity.rs` exists with `FidelityConfig`,
  `FidelityState`, `FidelityOutcome`, `FidelityError`, gain/scalar helpers and
  the pure `process` stage.
- `crates/touchpad-core/src/m11.rs` exists with `M11Profile` and the provisional
  M11 constants.
- `crates/touchpad-core/src/lib.rs` already exposes M11/fidelity APIs.
- `crates/touchpad-core/src/arbiter.rs` currently has only the optional
  `FidelityConfig` field plus `with_fidelity`, `fidelity_config`, and
  `is_fidelity_enabled`; M11 pointer routing/state integration is **not yet
  implemented**.
- `crates/touchpad-core/tests/m11_fidelity.rs` currently contains only a small
  public API smoke test and is far from the required M11 coverage.
- `crates/touchpad-core/src/m10.rs` has not been edited and must remain
  unchanged.

Known local baseline after the partial work:

- `cargo test -p touchpad-core m11` passed;
- `cargo test -p touchpad-core fidelity` passed;
- the current M11 integration smoke test passed.

These partial passes are not M11 completion.

## 3. Batch 1 — finish pure fidelity + profile tests

Review the current pure implementation against `M11_TASK.md` §§6–10 and fix
any mismatch. Required behavior:

- `FidelityConfig` validates every documented field/relationship:
  - `dead_zone_radius_mm`: finite and `> 0`;
  - `velocity_tau`: `> 0`;
  - `long_gap`: `> 0`;
  - `gain_x0_mm_per_s`: finite and `> 0`;
  - `gain_x1_mm_per_s`: finite and `> x0`;
  - `min_gain`: finite and `> 0`, `<= max_gain`;
  - `max_gain`: finite and `>= min_gain`;
  - `base_px_per_mm`: existing validated type;
  - `tracking_speed`: finite and `> 0`.
- first fidelity call:
  - anchor current timestamp;
  - fold entire delta into signed radial dead-zone accumulator `P`;
  - do not place first/pre-anchor displacement in `V_pending`;
  - do not fabricate elapsed time or velocity;
  - release `P` at initial velocity zero/min gain if radius is reached;
  - retain sub-radius `P`.
- duplicate timestamp (`dt == 0`):
  - fold delta into `P` and `V_pending` exactly once;
  - add zero time;
  - no division, no velocity update;
  - no dead-zone evaluation/flush on that frame;
  - next positive sample consumes accumulated duplicate displacement exactly
    once.
- positive elapsed time below long gap:
  - fold current delta into `P` and `V_pending`;
  - accumulate elapsed time;
  - `s = norm(V_pending) / t_acc`;
  - `alpha = 1 - exp(-t_acc / velocity_tau)`;
  - EMA update;
  - clear `V_pending` and `t_acc` after update;
  - evaluate signed radial dead zone and emit all `P` at release;
  - advance sample timestamp.
- long gap is inclusive (`dt >= long_gap`) and is checked before folding the
  gap-crossing delta:
  - discard gap-crossing delta;
  - clear `P`, `V_pending`, accumulated time and filtered velocity;
  - re-anchor to current timestamp;
  - return normal `Reanchored` and emit zero;
  - Arbiter pixel remainder must later be preserved on this path.
- signed radial dead zone must support cancellation/reversal/diagonal motion;
  slow consistent motion waits until radius and then releases all accumulated
  signed displacement.
- gain curve must implement the exact smoothstep formula in `M11_TASK.md` §8,
  be continuous, monotonic non-decreasing, finite, and bounded.
- scalar is isotropic and includes explicit tracking multiplier.
- runtime non-finite/overflow arithmetic is `FidelityError`; hold/re-anchor are
  normal outcomes.

Expand `crates/touchpad-core/tests/m11_fidelity.rs` substantially. Pure/profile
tests must cover at least:

- every configuration validation boundary above, including NaN/infinity where
  applicable and valid `min_gain == max_gain`;
- first-call full-motion preservation at min gain and exclusion from the next
  velocity numerator;
- first-call sub-radius hold;
- duplicate timestamps, including a duplicate frame that pushes `P` over the
  dead-zone radius but still must not flush;
- repeated zero-dt frames never fabricate velocity;
- long-gap at `long_gap - 1 ns`, exactly `long_gap`, and above it;
- signed cancellation, slow monotonic release, reversals and diagonals;
- smoothstep/gain continuity, monotonicity and bounds;
- isotropic scaling and tracking multiplier;
- runtime non-finite/overflow failure where constructible through public APIs;
- same constant physical motion at 60 Hz and 120 Hz after the same warm-up
  time: relative difference in filtered velocity, gain and scalar each
  `<= 1%`;
- `M11Profile` exact constants and inheritance of every exposed M10 value;
- M11 config is exactly M10 Arbiter config plus fidelity, not copied M7–M9
  constants.

Do not edit `m10.rs`.

## 4. Batch 2 — atomic Arbiter integration

Implement the full M11 integration described by `M11_TASK.md` §§5–9.

### State ownership

- Store one `FidelityState` inside `ArbiterState`.
- It must participate in the existing copy/draft/commit behavior of
  `Arbiter::frame`.
- Rejected frames must not partially mutate fidelity state or pixel remainder.
- Do not add a second pixel remainder. Existing `remainder_x_px` /
  `remainder_y_px` remain the only pointer remainder.

### Pointer routing

Create one narrow pointer-delta helper/mode switch shared by every committed
one-finger pointer movement path:

- candidate commitment (including the accumulated candidate displacement);
- committed continuation;
- final clean `Ended` movement;
- tracking-id replacement's old final committed motion;
- M8 tap-drag/drag-lock/follow-up committed pointer movement wherever it uses
  the same one-finger pointer machinery.

When `fidelity_config() == None`, execute the existing M10/M7 quantization
branch unchanged. Do not call fidelity code in that branch.

When fidelity is enabled:

- pass only committed normalized millimeter deltas plus current frame's
  monotonic timestamp to the fidelity stage;
- ownership/candidate/tap/two-finger decisions remain based on the original
  normalized millimeter positions before dead-zone/gain;
- `Hold` and `Reanchored` emit no pointer event and do not alter pointer
  remainder;
- `EmitScaledPixels` goes through existing truncation-toward-zero quantization
  with existing per-axis remainder;
- map `FidelityError` fail-closed to existing
  `ArbiterError::NonFinite { sequence }` and preserve frame atomicity.

### Lifecycle/reset ordering

Match `M11_TASK.md` §9 exactly:

- clean end: process valid final committed motion, then reset interaction
  fidelity and pixel remainder;
- tracking-id replacement: process old final committed movement if present,
  reset old interaction, then begin new contact with fresh fidelity state;
- discontinuity: cancel/reset before contact handling and emit no final pointer
  motion from the cancelled interaction;
- missing coordinates, second/extra contacts, other cancellation and
  `release_all`: discard pending fidelity motion and reset;
- long-gap re-anchor is not a lifecycle reset and preserves the existing pixel
  remainder;
- after a true interaction reset, timestamp/velocity/P/V/time and pointer
  remainder must not leak into the next interaction.

### Arbiter tests

Add/extend tests proving:

- fidelity-disabled M10 fixtures/decisions remain identical;
- first M11 commitment, continuation and final clean movement;
- exact per-axis remainder invariant and no epsilon drain;
- `Arbiter::remainder_px()` exposes committed M11 remainder;
- `Hold` leaves remainder unchanged;
- `Reanchored` preserves remainder;
- a rejected/runtime-error frame rolls back remainder and fidelity state;
- existing timestamp/sequence regression errors occur before fidelity and do
  not partially apply the rejected frame;
- clean end, replacement, discontinuity, cancellation and `release_all`
  ordering/reset semantics;
- tap, tap-drag, drag-lock, two-finger scroll/tap, and physical button
  ownership semantics are unchanged and remain pre-fidelity.

## 5. Batch 3 — CLI routing, replay and documentation

### CLI

Extend the existing mandatory `--profile` option. Accepted set is exactly:

`{m10-linear-v1, m11-fidelity-v1}`

Requirements:

- add no new takeover flag;
- infer no profile when `--profile` is absent;
- retain all five M10 opt-ins and all duplicate/malformed/unknown validation;
- `m10-linear-v1` remains mention-first baseline in help/errors;
- unknown-profile error must name the accepted set accurately;
- introduce/use a pure profile selection + banner/preflight helper that can be
  tested without entering real `takeover::run` side effects;
- M10 selection constructs exactly `M10Profile` config;
- M11 selection constructs exactly `M11Profile` config;
- M11 banner, emitted before any device/output/recorder/countdown/grab side
  effect, explicitly states:
  - experimental and uncalibrated;
  - not the default;
  - no macOS equivalence claim;
  - no live M11 validation has occurred;
  - all M10 safety opt-ins and 1..=300 second bound still apply.

Test accepted profiles, missing/duplicate profile, unknown profile text, pure
routing/banner, and preservation of all existing takeover opt-ins. Fake-backed
command tests only; no live takeover.

### Trace/replay

Add deterministic M11 trace/replay coverage/fixture as appropriate to the
current trace architecture. Cover first commit, low/high speed, duplicate
timestamps, reversal, diagonal movement, exact/over long gap, clean end, and a
fresh interaction. Direct synthetic frames and replay-derived frames must
produce identical M11 decisions.

### Documentation

Create `docs/M11_ACCEPTANCE.md` as a future user-run procedure, but do not
execute it.

Update applicable `README.md`, `DESIGN_V2.md`, `MILESTONES.md`, CLI help and
related docs so they clearly separate:

- M10 code approval from pending M10 live qualification;
- M10 live acceptance uses `m10-linear-v1` and still requires M6 calibration
  evidence plus 10/60/300-second user runs;
- M11 is experimental/provisional and opt-in;
- M11 code completion does not imply M11 live qualification;
- M11 requires a separate future M11-specific live acceptance;
- no macOS equivalence claim is made;
- M12 has not begun.

Do not falsely state that independent M11 review has passed until the review
file actually says so.

## 6. Required full gates

Before declaring implementation ready for review, run and fix until all pass:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Also explicitly verify/report:

- no new dependency was added;
- no new unsafe code was added and existing `#![forbid(unsafe_code)]` remains;
- changed non-generated text contains no credential/secret;
- no live command/hardware/output side effect was executed;
- `m10.rs` remains unchanged;
- M10/output remains live-unqualified;
- M11 remains live-unqualified pending separate future acceptance;
- no M12 implementation/artifact was added.

If test counts are documented, derive them from actual final gate output; do
not guess.

## 7. Completion boundary for this execution task

This execution task is complete when:

1. all implementation/test/doc requirements above match `M11_TASK.md`;
2. all four full gates pass;
3. M10 decisions remain output-compatible on the fidelity-disabled path;
4. no live side effects occurred;
5. the workspace is ready for an **independent M11 review**.

At that point stop. Do not create a self-approval and do not begin M12. The
reviewer will write findings to `reviews/M11_REVIEW.md`; any findings will be
handled in a subsequent repair execution against that review document.
