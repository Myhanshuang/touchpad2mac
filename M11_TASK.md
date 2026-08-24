# M11 Task: Experimental One-Finger Pointer Fidelity (`m11-fidelity-v1`)

Status: DRAFT / UNDER REVIEW. Implementation starts only after this task, the
design specification, and the implementation plan pass review.

M11 is experimental, opt-in only, never the default, and makes no macOS
equivalence claim.

## 1. Execution Boundary

Implementation and automated validation are offline/fake-backed. Unit tests,
configuration tests, `M11Profile` construction, Arbiter tests, pure CLI helper
tests, and trace/replay tests may execute M11 logic.

Implementation and automated tests must not execute a live M11 takeover
session, open or grab a real input device, create a real portal/libei session,
emit desktop input, change system settings, or run a live command. Only the
user may later choose a live bounded takeover.

## 2. Objective

Add a platform-independent one-finger pointer-fidelity stage for already
normalized millimeter input. The stage applies only to committed one-finger
pointer motion and provides:

- a signed radial jitter dead-zone;
- a monotonic, time-domain velocity estimate;
- a continuous bounded smoothstep gain curve;
- an explicit tracking-speed multiplier;
- the existing per-axis subpixel remainder behavior.

The existing `m10-linear-v1` path must remain output-compatible and follow its
current branch without passing through M11 fidelity logic.

## 3. Qualification and Acceptance

Three states must remain separate:

1. M11 code-complete: all offline gates pass and independent code review
   approves the implementation.
2. M10/output live qualification: remains pending until the user records the
   M6 relative-delta/pixel-scroll calibration evidence and passes the ordered
   10-second, 60-second, then 300-second acceptance using
   `m10-linear-v1`.
3. M11 live qualification: `m11-fidelity-v1` remains live-unqualified until a
   separate, later M11-specific user acceptance is written and passed. M10
   acceptance does not qualify M11.

`--output-qualified` remains an operator attestation, not measurement
evidence. M11 must not weaken or bypass the M6/M10 gate.

## 4. Existing Bounded Takeover Contract

The existing M10 command contract remains:

```text
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile PROFILE \
  --max-duration-seconds N
```

- `DEVICE` and `TRACE` remain mandatory explicit positional paths.
- All five opt-ins remain mandatory and independently validated.
- Confirmation remains the exact non-interactive text `TAKEOVER`.
- `N` remains an integer in `1..=300`.
- Missing, repeated, conflicting, malformed, overflow, unknown, or
  takeover-only flags on another command remain usage errors before side
  effects.
- The bounded loop, countdown, stop handling, cleanup order, exit codes, and
  no-daemon/no-persistence rules remain unchanged.

M11 adds no flag. It adds `m11-fidelity-v1` as a second accepted value of the
existing mandatory `--profile` option. The accepted set becomes exactly:

```text
{m10-linear-v1, m11-fidelity-v1}
```

`m10-linear-v1` remains the mention-first baseline and no profile is inferred
when `--profile` is absent.

## 5. Placement and Ownership

```text
normalized ContactFrame (Millimeters + Monotonic timestamp)
  -> existing M7-M9 ownership, candidate, tap, and scroll decisions
  -> committed one-finger pointer delta in millimeters
  -> M11 fidelity stage, only when FidelityConfig is present
  -> existing OutputEvent::PointerMove path
```

- Raw counts never enter M11. Raw-to-mm conversion remains at the existing
  platform boundary.
- Candidate commitment, tap/drag qualification, and two-finger competition
  remain based on raw normalized millimeters, before gain or dead-zone logic.
- `ArbiterConfig` gains `Option<FidelityConfig>`, defaulting to `None`, plus
  `with_fidelity`, `fidelity_config`, and `is_fidelity_enabled`.
- Fidelity-disabled pointer code must execute the existing quantization branch
  unchanged.
- `M11Profile` must obtain the M7-M9 configuration from
  `M10Profile::new()?.arbiter_config()` and add fidelity with
  `.with_fidelity(...)`. Do not copy M7-M9 constants and do not edit `m10.rs`.

## 6. Atomic State and Interface

Create `touchpad-core::fidelity`, containing typed configuration, runtime
state, errors, and a pure stage API. The intended shape is:

```text
FidelityConfig
FidelityState
  initialized / last_sample_timestamp
  pending_dead_zone_mm: signed (x, y)             // P
  pending_velocity_mm: signed (x, y)              // V_pending
  pending_velocity_time: Duration/f64 seconds     // t_acc
  filtered_velocity_mm_per_s

process(config, state, delta_mm, timestamp)
  -> Result<FidelityOutcome, FidelityError>

FidelityOutcome
  Hold
  EmitScaledPixels { x: f64, y: f64 }
  Reanchored
```

The exact Rust names may follow local naming conventions, but the state and
behavior above are required.

`FidelityState` is stored in `ArbiterState`, not as a separately mutated field
on `Arbiter`. `Arbiter::frame` copies it into the existing frame draft and
commits it only with the rest of `ArbiterState`. A rejected frame rolls back
all fidelity state.

The fidelity stage returns finite scaled pixel deltas but does not own an
output sink. Arbiter uses the existing `remainder_x_px` and `remainder_y_px`
fields and existing truncation-toward-zero quantization. Therefore:

- `Arbiter::remainder_px()` exposes the committed fidelity remainder;
- a rejected frame leaves `remainder_px()` unchanged;
- lifecycle reset clears the remainder through the existing state reset;
- no second or hidden remainder accumulator is added.

## 7. Exact Timing and Dead-Zone Algorithm

The dead-zone and velocity estimator use separate displacement accumulators:

- `P`: signed 2D millimeter displacement waiting for dead-zone release;
- `V_pending`: signed 2D displacement waiting for a valid velocity sample;
- `t_acc`: positive elapsed time paired with `V_pending`.

All comparisons use checked monotonic time. Arbiter's existing sequence and
timestamp regression checks run before fidelity.

### 7.1 First fidelity call

The first call normally carries the whole M7 candidate displacement. Its
pre-commit elapsed interval is not available to the fidelity stage.

1. Anchor `last_sample_timestamp` to the current frame.
2. Fold the entire accepted displacement into `P` so motion is not lost.
3. Do not add that pre-anchor displacement to `V_pending` and do not fabricate
   a velocity sample.
4. Use the initial filtered velocity `0`, hence `min_gain`, if `P` releases.
5. Evaluate the dead-zone and either hold `P` or emit all of it.

This preserves the first committed displacement without causing a delayed
velocity spike on the next frame.

### 7.2 Duplicate timestamp (`dt == 0`)

1. Fold the frame delta into both `P` and `V_pending`.
2. Add zero to `t_acc`.
3. Do not divide and do not update filtered velocity.
4. Do not evaluate the dead-zone on this frame. The dead-zone is evaluated
   only after a velocity update — on a positive-`dt` frame, or on the first
   call at its pre-anchor min gain — so on a duplicate frame `P` merely
   accumulates and is not flushed.

Duplicate-timestamp displacement participates exactly once in the next valid
velocity sample.

### 7.3 Positive elapsed time (`0 < dt < long_gap`)

1. Fold the frame delta into `P` and `V_pending`.
2. Add `dt` to `t_acc`.
3. Compute `s = norm(V_pending) / t_acc`.
4. Compute `alpha = 1 - exp(-t_acc / velocity_tau)`.
5. Update `v = alpha * s + (1 - alpha) * v_previous`.
6. Clear `V_pending` and `t_acc` after the velocity update.
7. Evaluate the dead-zone. If `norm(P) >= radius`, scale and emit all of `P`,
   then clear `P`; otherwise emit nothing and retain `P`.
8. Advance `last_sample_timestamp`.

The dead-zone is signed and radial. Oscillation cancels algebraically. Slow
consistent motion is delayed until the radius is reached, then released; a
below-radius remainder may be discarded only at a lifecycle reset or long-gap
re-anchor.

### 7.4 Long gap (`dt >= long_gap`)

The boundary is inclusive. Check it before folding the gap-crossing delta.

1. Discard the gap-crossing displacement.
2. Clear `P`, `V_pending`, `t_acc`, and filtered velocity.
3. Preserve Arbiter's existing subpixel pixel remainder because the same
   committed output interaction continues.
4. Re-anchor the timestamp to the gap-crossing frame.
5. Emit zero.

A long gap is a normal `Reanchored` outcome, not an error or lifecycle reset.

## 8. Gain and Quantization

For filtered velocity `v`:

```text
t = clamp((v - x0) / (x1 - x0), 0, 1)
w = t * t * (3 - 2 * t)
gain = min_gain + (max_gain - min_gain) * w
scalar = base_px_per_mm * gain * tracking_speed
```

The scalar is isotropic. It must be finite, continuous, monotonic
non-decreasing, and bounded by the configured min/max gains.

For each axis Arbiter preserves the existing invariant:

```text
total = prior_remainder + emitted_mm_axis * scalar
integer_output = trunc(total)
new_remainder = total - integer_output
```

The remainder stays in `(-1, 1)`. There is no epsilon drain.

## 9. Lifecycle Reset Ordering

- Clean end: process valid final committed motion, then reset.
- Tracking-id replacement: process the old contact's final committed motion
  when present, reset old fidelity state, then begin the new contact with
  fresh state.
- Discontinuity: cancel and reset before contact handling; emit no final
  pointer motion from the cancelled interaction.
- Other cancellation and `release_all`: discard all pending fidelity motion,
  then reset.

Frame-driven reset observability uses the existing structured
`InteractionFinished` or `InteractionCancelled` diagnostic and lifecycle
transition; cancellation diagnostics retain their reason.
`release_all` is observable through the explicit API call/outcome and returns
no `FrameDecision`, so M11 must not add a hidden logger or change that API only
to emit a diagnostic. Long-gap re-anchor emits no lifecycle diagnostic.

After reset, no timestamp, velocity, `P`, `V_pending`, `t_acc`, or subpixel
remainder may leak into a new interaction.

## 10. Configuration (`m11-fidelity-v1`)

All values are typed, finite, validated, versioned, documented in one source
location, and never loaded from KDE/libinput:

| Parameter | Value | Validation |
| --- | ---: | --- |
| `dead_zone_radius_mm` | `0.09 mm` | finite, `> 0` |
| `velocity_tau` | `20 ms` | finite duration, `> 0` |
| `long_gap` | `150 ms` | `> 0`; inclusive boundary |
| `gain_x0_mm_per_s` | `50.0` | finite, `> 0` |
| `gain_x1_mm_per_s` | `600.0` | finite, `> x0` |
| `min_gain` | `1.0` | finite, `> 0`, `<= max_gain` |
| `max_gain` | `2.0` | finite, `>= min_gain` |
| `base_px_per_mm` | `10.0` | existing validated type |
| `tracking_speed` | `1.0` | finite, `> 0` |

`FidelityConfigError` covers invalid construction. Runtime
`FidelityError` covers only fidelity arithmetic that becomes non-finite or
overflows; Arbiter maps it fail-closed to `ArbiterError::NonFinite` with the
current sequence. Existing Arbiter timestamp/sequence regression errors stay
outside fidelity. Dead-zone hold, reset, and re-anchor are normal outcomes.

## 11. CLI Routing and Banner

Profile selection, configuration construction, and banner selection must be a
pure helper that can be tested without entering `takeover::run` side effects.

For `m11-fidelity-v1`, the helper returns an explicit banner stating:

- experimental and uncalibrated;
- not the default;
- no macOS equivalence claim;
- no live M11 validation has occurred;
- M10 safety opt-ins and duration bound still apply.

The banner is written before device, output, recorder, countdown, or grab side
effects. Automated tests call only the pure helper and fake-backed command
paths, never a live session.

## 12. Required Offline Tests

- Every `FidelityConfig` validation boundary.
- First commit preserves full motion at `min_gain` without entering the next
  velocity numerator.
- Duplicate timestamps fold signed displacement and enter the next positive
  sample exactly once.
- Zero timestamps never divide or fabricate velocity.
- Existing timestamp/sequence regression returns the existing Arbiter error,
  emits zero, and leaves no partial fidelity draft.
- Long-gap tests at `long_gap - 1 ns`, exactly `long_gap`, and above it.
- Signed cancellation, slow monotonic release, reversals, and diagonals.
- Smoothstep continuity, monotonicity, bounds, and finite results.
- Constant physical motion at 60 Hz and 120 Hz: after the same warm-up time,
  relative differences in filtered velocity, gain, and scalar are each
  `<= 1%`. Do not require byte-identical pointer events across rates.
- Tracking multiplier and isotropic scaling.
- Exact per-axis remainder invariant, no epsilon drain, committed exposure via
  `remainder_px`, and rollback after a rejected frame.
- Clean end, replacement, discontinuity, cancellation, and `release_all`
  ordering with no stale state.
- Tap, tap-drag, drag-lock, two-finger, and physical-button ownership remain
  based on pre-fidelity behavior.
- Fidelity-disabled M10 regression fixtures produce identical decisions.
- `M11Profile` inherits every M10 config value and only adds fidelity.
- CLI accepted set, duplicate/missing opt-ins, unknown profile message, pure
  profile routing, and experimental banner.
- Direct synthetic frames and trace/replay frames produce identical M11
  decisions.
- No test opens hardware, constructs a real portal/libei session, emits live
  input, sleeps for timing, or changes system state.

## 13. Quality Gates

Run and report:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Also verify:

- no new dependency or unsafe code;
- changed non-generated text contains no credential;
- no live command ran;
- M10/output status remains live-unqualified;
- M12 work did not begin.

## 14. Exit Criteria

M11 is code-complete only when the implementation matches this contract, all
offline gates pass, independent code review approves it, M10 decisions remain
output-compatible, and documentation clearly separates M10 acceptance from a
later M11-specific live acceptance. Code completion does not confer live
qualification.
