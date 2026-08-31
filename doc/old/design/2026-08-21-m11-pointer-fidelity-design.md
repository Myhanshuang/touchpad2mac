# M11 Pointer Fidelity Design Specification

- **Milestone:** M11
- **Status:** Draft / under review
- **Authority:** `M11_TASK.md`
- **Date:** 2026-08-21

This document explains how the M11 contract fits the current code. It does not
authorize implementation by itself.

## 1. Design Goals

1. Preserve the fidelity-disabled `m10-linear-v1` decision stream.
2. Add one platform-independent stage for committed one-finger motion.
3. Keep raw-mm gesture ownership outside fidelity.
4. Make every mutable fidelity value part of Arbiter's existing atomic draft.
5. Use time-domain smoothing so gain does not depend on report rate.
6. Preserve the existing pixel remainder and reset behavior.
7. Expose M11 only as an explicit value of the existing bounded
   `--profile` option.

M10/output remains live-unqualified pending M6 calibration plus its ordered
M10 acceptance. M11 has a separate later live acceptance.

## 2. Files and Responsibilities

### New

- `crates/touchpad-core/src/fidelity.rs`
  - `FidelityConfig` and `FidelityConfigError`
  - `FidelityState`
  - `FidelityOutcome` and `FidelityError`
  - pure dead-zone, velocity, gain, and scaling logic
- `crates/touchpad-core/src/m11.rs`
  - versioned `M11Profile`
  - the single set of provisional M11 constants
- `crates/touchpad-core/tests/m11_fidelity.rs`
  - public API and end-to-end Arbiter contract tests
- `docs/M11_ACCEPTANCE.md`
  - write-only future user procedure; not executed during implementation

### Modified

- `crates/touchpad-core/src/arbiter.rs`
  - optional config, draft-owned runtime state, and one mode switch in pointer
    emission
- `crates/touchpad-core/src/lib.rs`
  - module declarations and public re-exports
- `apps/touchpadctl/src/args.rs`
  - accepted profile set and help/error text
- `apps/touchpadctl/src/cmd/takeover.rs`
  - pure profile selection/banner helper and selected config routing
- `apps/touchpadctl/tests/cli.rs`
  - public CLI contract tests
- `crates/touchpad-trace/tests/fixtures/` and relevant replay tests
  - deterministic M11 trace coverage
- `README.md`, `DESIGN_V2.md`, and `MILESTONES.md`
  - honest M10/M11 status and profile documentation

`crates/touchpad-core/src/m10.rs` is not modified.

## 3. Existing Arbiter Integration

The current `Arbiter` contains only `config` and `state`. `frame()` copies
`self.state` to a local draft, computes the decision, and assigns the draft
back only after processing succeeds. M11 keeps that structure.

```text
Arbiter::frame
  -> existing validation and regression checks
  -> draft = self.state
  -> draft.handle_contacts
       -> commit_pointer       (candidate crosses threshold)
       -> emit_position        (committed continuation/final movement)
       -> emit_pointer_delta   (new narrow mode switch)
            fidelity None -> existing scale + quantize branch
            fidelity Some -> fidelity::process -> existing quantize
  -> self.state = draft
```

The new helper name is illustrative. The required property is one narrow
switch shared by candidate commitment, continued motion, final clean motion,
and M8 follow-up/locked motion.

### 3.1 Configuration

`ArbiterConfig` adds:

```rust
fidelity: Option<FidelityConfig>
```

and the established-style methods:

```rust
with_fidelity(FidelityConfig) -> Self
fidelity_config() -> Option<&FidelityConfig>
is_fidelity_enabled() -> bool
```

`ArbiterConfig::new` sets `None`. No fidelity function is called on that
branch.

### 3.2 Runtime state

`ArbiterState` adds one copyable `FidelityState`:

```text
last_sample_timestamp: Option<Monotonic>
pending_dead_zone_x_mm: f64
pending_dead_zone_y_mm: f64
pending_velocity_x_mm: f64
pending_velocity_y_mm: f64
pending_velocity_seconds: f64
filtered_velocity_mm_per_s: f64
```

The existing `remainder_x_px` and `remainder_y_px` remain the only pixel
remainder. `FidelityState` does not duplicate them.

Because `ArbiterState` is copied into the frame draft, fidelity arithmetic and
remainder updates commit together. A runtime error discards both.

## 4. Fidelity Module Boundary

The stage receives only a signed millimeter delta and the current monotonic
timestamp:

```rust
process(
    config: &FidelityConfig,
    state: &mut FidelityState,
    delta: FidelityDeltaMm,
    timestamp: Monotonic,
) -> Result<FidelityOutcome, FidelityError>
```

Suggested outcomes:

```rust
enum FidelityOutcome {
    Hold,
    EmitScaledPixels { x: f64, y: f64 },
    Reanchored,
}
```

The output values include base scale, curve gain, and tracking multiplier but
not the prior pixel remainder. Arbiter passes them to its existing `quantize`
and `push_move` functions.

The stage never sees raw counts, contacts, taps, scroll ownership, output
sinks, portal/libei objects, or wall-clock time.

## 5. Configuration

`FidelityConfig::new` validates all fields and exposes read-only accessors:

| Field | M11 value | Rule |
| --- | ---: | --- |
| dead-zone radius | `0.09 mm` | finite and positive |
| velocity time constant | `20 ms` | positive |
| long gap | `150 ms` | positive; inclusive comparison |
| velocity x0 | `50 mm/s` | finite and positive |
| velocity x1 | `600 mm/s` | finite and greater than x0 |
| minimum gain | `1.0` | finite, positive, <= maximum |
| maximum gain | `2.0` | finite and >= minimum |
| base scale | `10 px/mm` | existing validated type |
| tracking multiplier | `1.0` | finite and positive |

Duration values use `Duration`; arithmetic converts checked elapsed duration
to finite `f64` seconds. There is no public fixed `velocity_alpha` field.

`M11Profile::new` constructs the fidelity config and retains the validated
`M10Profile`. Its Arbiter config is:

```rust
m10_profile.arbiter_config().with_fidelity(fidelity_config)
```

This prevents M7-M9 constant drift.

## 6. Timing State Machine

Definitions:

- `P`: signed dead-zone displacement.
- `V`: signed velocity-sample displacement.
- `T`: positive elapsed seconds paired with `V`.
- `v`: filtered scalar velocity.

Arbiter rejects sequence/timestamp regression before calling the stage.

### 6.1 Uninitialized

The first call is usually the full M7 candidate displacement:

1. Set `last_sample_timestamp = now`.
2. Add the full delta to `P`.
3. Leave `V` and `T` zero because the pre-commit interval is unknown.
4. Keep `v = 0`.
5. Apply the dead-zone. A release uses minimum gain.

The first accepted movement is not lost and cannot inflate the next velocity
sample.

### 6.2 Duplicate time

For `dt == 0`:

1. Add delta to `P` and `V`.
2. Do not change `T` or `v`.
3. Apply the dead-zone using the current `v`.

The next positive sample includes the duplicate displacement once.

### 6.3 Positive time below long gap

For `0 < dt < long_gap`:

1. Add delta to `P` and `V`.
2. Add `dt` to `T`.
3. Compute `s = hypot(V.x, V.y) / T`.
4. Compute `alpha = 1 - exp(-T / tau)`.
5. Compute `v = alpha * s + (1 - alpha) * previous_v`.
6. Clear `V` and `T`.
7. Apply the dead-zone using the new `v`.
8. Set `last_sample_timestamp = now`.

Every intermediate value is checked finite before draft commit.

### 6.4 Inclusive long gap

For `dt >= long_gap`, before folding delta:

1. discard the gap-crossing delta;
2. clear `P`, `V`, `T`, and `v`;
3. preserve Arbiter's pixel remainder;
4. set `last_sample_timestamp = now`;
5. return `Reanchored` with zero movement.

This is a normal policy result, not an error or lifecycle reset.

## 7. Dead-Zone and Gain

After timing processing, the signed vector `P` is radially gated:

- if `hypot(P.x, P.y) < 0.09 mm`, return `Hold`;
- otherwise copy all of `P`, clear `P`, calculate the scalar, and return the
  scaled values.

Signed oscillation cancels. A below-radius vector is retained across ordinary
frames but discarded by long-gap or lifecycle reset.

The gain curve is:

```text
t = clamp((v - x0) / (x1 - x0), 0, 1)
w = t*t*(3 - 2*t)
gain = min_gain + (max_gain - min_gain)*w
scalar = base_px_per_mm * gain * tracking_speed
```

The same scalar applies to both axes. Validation plus runtime finite checks
keep the output bounded and finite.

## 8. Quantization and Atomicity

For each emitted scaled axis, Arbiter uses:

```text
total = scaled + old_remainder
emitted = trunc(total)
new_remainder = total - emitted
```

The existing `Arbiter::remainder_px()` accessor therefore reports M11 state
without an API addition. Tests must prove:

- a committed M11 frame advances it;
- `Hold` leaves it unchanged;
- `Reanchored` preserves it;
- a runtime error rolls it back;
- lifecycle reset clears it.

No epsilon drain is allowed.

## 9. Errors

`FidelityConfigError` is construction-only and includes a typed variant for
each invalid field or field relationship.

`FidelityError` is runtime-only and represents non-finite or overflowing
fidelity arithmetic. Arbiter maps it to the existing
`ArbiterError::NonFinite { sequence }`.

The following are not `FidelityError`:

- timestamp and sequence regression, which remain existing Arbiter errors;
- dead-zone hold;
- long-gap re-anchor;
- lifecycle reset.

No runtime error may commit events, fidelity state, last position, or pixel
remainder.

## 10. Lifecycle Ordering

The stage is reset through the same helpers that clear one-finger interaction
state:

- clean end: `emit_position` processes final coordinates, then
  `finish_interaction` clears fidelity and pixel remainder;
- replacement: old final committed motion is processed when present, then old
  state is cleared, then `begin_candidate` starts fresh state;
- discontinuity: the existing pre-contact cancellation clears state and no
  final pointer movement is processed;
- missing coordinates, extra contacts, other cancellation, and `release_all`:
  pending motion is discarded and state is cleared.

No new logging subsystem is introduced. Existing
`InteractionFinished`/`InteractionCancelled` diagnostics and lifecycle
transitions make frame-driven reset paths observable; cancellation diagnostics
retain their reason. `release_all` remains observable through its explicit
call and result. Long-gap re-anchor does not emit a lifecycle diagnostic.

## 11. CLI Design

`args.rs` accepts exactly two profile strings and continues to reject a
missing or duplicate `--profile`.

`takeover.rs` gains a pure helper used before external preparation:

```text
select_takeover_profile(name)
  -> validated ArbiterConfig
  -> display name/details
  -> optional warning banner
```

For M11 the banner says experimental, uncalibrated, non-default, no macOS
equivalence, and no live M11 validation. Tests invoke the helper directly and
through fake command dependencies. They do not enter a real takeover.

All existing M10 opt-ins, countdown, bounded loop, interruption handling,
resource order, cleanup, and exit priorities remain unchanged.

## 12. Test Design

### Pure fidelity tests

- config validation table;
- first-call minimum gain and exclusion from the velocity numerator;
- duplicate folding followed by one positive-time update;
- exact long-gap boundaries;
- signed cancellation and slow release;
- smoothstep continuity, monotonicity, and bounds;
- isotropic output and tracking multiplier;
- non-finite/overflow failure.

### Sample-rate test

Feed the same constant physical velocity over the same warm-up duration at 60
Hz and 120 Hz. Compare filtered velocity, gain, and scalar after warm-up; each
relative difference must be <= 1%. Pointer-event byte equality is not required
because dead-zone release and integer quantization phases may differ.

### Arbiter tests

- disabled path decision equality with existing M10 fixtures;
- first commitment, continuation, final clean motion, and M8 locked movement;
- atomic rollback including `remainder_px`;
- reset/replacement/discontinuity ordering;
- unchanged tap/two-finger/button ownership;
- existing regression errors remain unchanged.

### Profile and CLI tests

- M11 inherits every exposed M10 value;
- accepted profile set and error text;
- no inferred default and duplicate rejection;
- pure banner/preflight ordering before fake side effects;
- all M10 mandatory opt-ins remain mandatory.

### Replay tests

A deterministic M11 fixture covers first commit, low/high speed, duplicate
timestamps, reversal, diagonal movement, exact/over long gap, clean end, and a
new interaction. Direct and replay-derived frames produce identical decisions.

All tests are offline/fake-backed and may execute M11 logic. None may touch a
real device, portal/libei, or desktop output.

## 13. Documentation and Acceptance

Implementation updates README/help/design/milestone status and adds
`docs/M11_ACCEPTANCE.md` without executing it.

Documentation must state:

- M10 acceptance still uses `m10-linear-v1` and remains pending;
- M11 needs a separate later user-run acceptance;
- both profiles retain the bounded takeover safety contract;
- M11 is provisional and makes no macOS equivalence claim;
- code completion is not live qualification.

Stop after M11. Do not begin M12.
