# M12 Task — Scroll Fidelity and Momentum

Authority: `PHASE2_PLAN.md` M12. M11 remains code-complete/review-approved but
live-unqualified. M12 implementation/testing is offline/fake-backed; do not
run real takeover while implementing or reviewing this milestone.

## Goal

Extend the existing M9 two-finger pixel scroll with a platform-independent,
time-domain fidelity stage and software momentum while preserving M9 ownership,
M10 bounded takeover safety, and the M11 pointer path.

## Required architecture

- Add a pure `scroll_fidelity` module with validated config/state/outcomes.
- Estimate two-finger centroid velocity using `ContactFrame.monotonic_timestamp`.
- Apply a continuous bounded smoothstep gain to committed scroll deltas.
- Add deterministic axis lock with hysteresis; diagonal intent must remain
  available when neither axis clearly dominates.
- Detect direction reversal and reset the velocity/lock history that would
  otherwise produce an acceleration burst.
- On clean finger release, start momentum only when the filtered scaled
  velocity exceeds the configured start threshold. Keep the existing scroll
  lifecycle open while momentum is active.
- Momentum is driven by an explicit `Arbiter::tick(Monotonic)`/equivalent pure
  time advance; never read wall-clock time inside core. Decay exponentially,
  preserve the existing per-axis scroll pixel remainder, and emit one
  `ScrollEnd` when velocity falls below the stop threshold.
- Any new touch/contact cluster, physical/synthetic button ownership,
  discontinuity, replacement/cancellation, explicit `release_all`, opposite
  direction scroll, output failure, or device/runtime shutdown cancels
  momentum immediately and closes the scroll lifecycle exactly once.
- Add `M12Profile` by inheriting `M11Profile`/M10 values and only adding scroll
  fidelity. Do not edit `m10.rs` or duplicate M7–M11 constants.
- Add CLI profile `m12-scroll-v1`; no new takeover safety flag and no inferred
  default. Existing M10/M11 profile behavior must remain identical.
- Runtime/bridge may drive momentum ticks only through fake/offline tests in
  this milestone. Live acceptance is documented but not executed.

## Provisional versioned parameters

- velocity tau: 30 ms
- gain x0/x1: 25 / 450 mm/s
- min/max gain: 1.0 / 1.75
- axis-lock engage ratio: 2.5
- axis-lock release ratio: 1.5
- momentum decay tau: 325 ms
- momentum start speed: 35 mm/s (centroid speed before px/mm scaling)
- momentum stop speed: 6 mm/s
- momentum tick cap: 16 ms per integration step (larger elapsed time is split)

These are provisional engineering constants, not Apple/macOS parameters.

## Tests / exit

Test config boundaries, 60/120 Hz rate stability, gain bounds, axis-lock
hysteresis, reversal, momentum start/decay/end, cancellation matrix, remainder
invariant, rejected-frame/tick rollback, M9/M10/M11 compatibility, bridge tick
fail-stop, CLI routing, and fake bounded-loop ticking. Run all workspace gates.
Code-complete != live-qualified.
