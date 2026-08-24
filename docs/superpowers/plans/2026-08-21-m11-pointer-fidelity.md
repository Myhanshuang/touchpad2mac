# M11 Pointer Fidelity Implementation Plan

> **For the dsh implementation worker:** implement one batch at a time using
> test-first development. Exit after the batch report; Codex reviews before
> authorizing the next batch.

**Goal:** Implement the opt-in `m11-fidelity-v1` one-finger pointer-fidelity
profile while preserving `m10-linear-v1` output behavior and all bounded
takeover safety rules.

**Architecture:** A new pure `touchpad-core::fidelity` module processes only
committed millimeter pointer deltas. Its runtime state lives in the existing
`ArbiterState` frame draft, while Arbiter retains ownership of lifecycle,
pixel remainder, event ordering, and atomic commit.

**Constraints:** Offline/fake-backed work only; no live takeover, real device,
portal/libei session, desktop emission, system setting change, new dependency,
or new unsafe code. The workspace has no usable Git history, so use a
timestamped backup under `/tmp` and never create a backup inside the repo.

---

## Batch 1: Core Fidelity and M11 Profile

**Files**

- Create `crates/touchpad-core/src/fidelity.rs`
- Create `crates/touchpad-core/src/m11.rs`
- Modify `crates/touchpad-core/src/lib.rs`
- Add focused public tests in `crates/touchpad-core/tests/m11_fidelity.rs`

Implement the typed `FidelityConfig`, `FidelityState`,
`FidelityOutcome`, and `FidelityError` described by `M11_TASK.md`.
The state must keep separate signed dead-zone and velocity accumulators.
Implement first-call anchoring, duplicate folding, time-domain EMA,
inclusive long-gap re-anchor, smoothstep gain, isotropic tracking multiplier,
and finite-output checks.

Use the exact provisional constants:

```text
dead zone 0.09 mm
velocity tau 20 ms
long gap 150 ms (dt >= threshold)
x0/x1 50/600 mm/s
gain 1.0..2.0
base 10 px/mm
tracking multiplier 1.0
```

`M11Profile` must derive its base Arbiter config from
`M10Profile::new()?.arbiter_config().with_fidelity(...)`; do not edit
`m10.rs` or copy M7-M9 constants.

Tests cover config rejection, first displacement at minimum gain without a
later speed spike, duplicate timestamps entering the next positive sample
once, long-gap boundaries, signed cancellation, gain bounds, diagonals,
overflow failure, and 60/120 Hz velocity/gain/scalar agreement within 1%.

Run:

```text
cargo test -p touchpad-core fidelity
cargo test -p touchpad-core m11
cargo fmt --all -- --check
cargo clippy -p touchpad-core --all-targets --locked -- -D warnings
```

The batch report lists changed files, test results, and deviations. Then dsh
exits for review.

## Batch 2: Atomic Arbiter Integration

**Files**

- Modify `crates/touchpad-core/src/arbiter.rs`
- Extend `crates/touchpad-core/tests/m11_fidelity.rs`
- Modify `crates/touchpad-core/src/lib.rs` only if exports need correction

Add default-off fidelity config accessors and store `FidelityState` inside
`ArbiterState`. Route every committed one-finger delta through one narrow
mode switch shared by first commit, continuation, final clean movement, and
M8 follow-up/locked motion. The disabled branch must retain the existing
scale/quantize behavior.

Arbiter remains responsible for the existing pixel remainder. A fidelity
runtime error must discard the frame draft, including fidelity state and
remainder. Preserve existing regression semantics, tap/two-finger ownership,
button ordering, and fail-stop behavior.

Reset ordering must match the current Arbiter:

- clean end processes valid final motion, then clears;
- replacement processes old final motion when present, clears old state, then
  begins the new contact;
- discontinuity cancels before contact handling with no final pointer motion;
- cancellation and `release_all` discard pending motion;
- long-gap re-anchor preserves the existing pixel remainder.

Tests prove M10 decision compatibility, M11 first/continued/final movement,
`Arbiter::remainder_px()` commit and rollback, lifecycle ordering, no stale
state, unchanged tap/drag/scroll/button competition, and existing regression
errors.

Run:

```text
cargo test -p touchpad-core
cargo fmt --all -- --check
cargo clippy -p touchpad-core --all-targets --locked -- -D warnings
```

The batch report includes the exact call sites changed and evidence that the
disabled branch remains compatible. Then dsh exits for review.

## Batch 3: Profile Routing, Replay, and Documentation

**Files**

- Modify `apps/touchpadctl/src/args.rs`
- Modify `apps/touchpadctl/src/cmd/takeover.rs`
- Modify `apps/touchpadctl/tests/cli.rs`
- Add an M11 fixture under `crates/touchpad-trace/tests/fixtures/`
- Extend the relevant trace/Linux replay tests
- Update `README.md`, `DESIGN_V2.md`, and `MILESTONES.md`
- Create `docs/M11_ACCEPTANCE.md`

Extend the existing mandatory `--profile` accepted set to exactly
`{m10-linear-v1, m11-fidelity-v1}`. Add no flag and infer no default.
Preserve duplicate/missing opt-in errors and every M10 bounded takeover rule.

Introduce a pure profile-selection/banner helper that constructs the selected
Arbiter config before external preparation. The M11 banner must say
experimental, uncalibrated, non-default, no macOS equivalence, and no live M11
validation. Test it through pure/fake paths only.

Add one deterministic replay fixture covering low/high speed, duplicates,
reversal, diagonal movement, long-gap re-anchor, clean end, and a new
interaction. Direct and replay-derived frames must produce identical
decisions.

Documentation must clearly separate:

- pending M10 acceptance using `m10-linear-v1` plus M6 calibration;
- future, separate M11 acceptance using `m11-fidelity-v1`;
- offline code completion from live qualification.

`docs/M11_ACCEPTANCE.md` is written but not executed.

Run:

```text
cargo test -p touchpadctl
cargo test -p touchpad-linux -p touchpad-trace
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Then dsh exits with a file/test/status report for review.

## Batch 4: Full Gates and Review Corrections

After Codex reviews Batches 1-3, dsh fixes only the reported findings. When
the review is clean, run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Also verify:

- no new dependency and `#![forbid(unsafe_code)]` remains effective;
- changed non-generated text contains no credential;
- no live command or real side effect ran;
- M10/output remains live-unqualified;
- M11 remains live-unqualified pending its own later user acceptance;
- no M12 artifact or implementation was added.

The final handoff records changed files, exact parameters, test counts,
compatibility evidence, remaining risks, and the separate M10/M11 acceptance
states. Only then may M11 be marked code-complete; it is not marked
live-qualified.
