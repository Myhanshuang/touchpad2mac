# M8 Review — Tap, Tap-and-Drag, Sticky Drag Lock (Offline)

Date: 2026-08-17  
Decision: **REJECTED — repair M8; do not start M9**

The explicit configuration, monotonic tap timing, maximum-displacement tracking, immediate follow-up press, sticky lock phases, source-aware physical/synthetic model, cleanup integration, offline replay seam, documentation, and broad deterministic tests are good. Independent fmt, clippy, and all 632 tests pass. Four untested boundary defects/mismatches remain; the first two can produce an invalid desktop button stream.

## Blocking findings

### R1 — Critical: final-`Ended` pointer commitment does not update tap/drag ownership

In the candidate release branch, a contact that first reaches the M7 pointer threshold in its final `Ended` frame calls `commit(...)` but does not call the M8 pointer-commit ownership update (`note_pointer_commit`). The active-frame commitment branch does call it.

Consequences:

- A first-tap candidate whose pointer threshold is less than or equal to its tap movement limit can emit a `PointerMove` and then still qualify as a tap in the same release decision. Because the button multiplexer orders downs before motion and ups after motion, the result may be `ButtonDown, PointerMove, ButtonUp` instead of pointer-only output. This violates the single-owner rule and the explicit invariant that one contact cannot produce both ordinary pointer movement and a tap click.
- A tap-and-drag contact that first crosses the pointer threshold in its final frame emits movement but leaves `drag_committed == false`; sticky drag lock therefore fails to engage and an up is emitted.
- A locked continuation that first crosses the threshold in its final frame may emit movement and then be misclassified as a qualifying unlock tap when the tap movement limit is wider than the pointer threshold.

Centralize pointer commitment side effects so active-frame and final-Ended commitment cannot diverge. Add exact event/phase tests for all three cases above, including equality at the motion threshold and a tap movement limit wider than the pointer threshold. Assert that pointer-only final commitment emits no synthetic button pair, final-frame tap-drag commitment enters lock without an up, and final-frame locked continuation remains locked.

### R2 — Critical: discontinuity plus simultaneous physical release can lose the aggregate up

Discontinuity cancellation runs before the frame's physical source is updated. If synthetic drag/lock and physical left are both held, and the discontinuity frame also reports physical left released, `cancel_tap_policy` clears synthetic state while still seeing the old physical-held value and therefore records no synthetic up. The later physical-up path suppresses its up because `synthetic_prev` was true. The post-frame aggregate is false but no `ButtonUp` is emitted.

In debug/test builds the final `simulate_wire` assertion should panic for this missing matrix entry; without debug assertions the release is silently lost, leaving the desktop button stream stuck down.

Repair the multiplexer/cancellation ordering so output is derived from a coherent sequence of source transitions, not stale source snapshots. Add a stateful exact-wire regression:

1. enter sticky synthetic lock;
2. press physical left while synthetic remains held (no duplicate down);
3. process one `discontinuity=true` frame with physical left now false;
4. require exactly one aggregate `ButtonUp`, no panic, both sources false, lock cancelled, and repeated cleanup/release producing no unmatched up.

Also table-test simultaneous physical transitions with synthetic cancellation caused by discontinuity, extra contacts, and missing active coordinates. Every case must finish with the emitted wire state equal to the post-frame aggregate in debug and release semantics.

### R3 — High: a discontinuity frame can immediately seed a new tap candidate

The discontinuity prelude cancels existing tap policy, but `handle_contacts` may then process a `Began` contact from that same discontinuity frame and start `FirstTapCandidate`. A later small/quick Ended frame can emit a click even though the candidate began across a stream discontinuity. This is unsafe: the runtime cannot know the real touch-down time or movement before the recovered boundary, and M8 explicitly requires discontinuous sequences to produce no synthetic click.

Preserve M7's ability to re-anchor a pointer candidate from a recovered frame, but make the tap family ineligible for any contact seeded by `discontinuity=true`. Cover a fresh arbiter discontinuity+Began followed by Ended, and an open follow-up window receiving discontinuity+Began: neither may emit a tap click or immediate tap-and-drag down. A later genuinely new Began after that contact ends may start tap policy normally.

### R4 — Medium: follow-up expiry uses saturating addition while the public contract claims checked timing

Follow-up expiry computes `completed.saturating_add(max_tap_drag_gap)`. M8_TASK and DESIGN_V2 both require checked duration arithmetic. The existing `Monotonic::duration_since`/`checked_add` APIs make it unnecessary to hide overflow through saturation.

Compare checked elapsed time with the configured gap (preferred), or explicitly handle `checked_add` failure with a documented deterministic policy. Add near-`u64::MAX` timestamp tests that prove equality is accepted, strictly greater expires, and no overflow is silently converted into a different state transition. Keep timestamp regression handling unchanged.

## Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 632 tests, 0 failed.
- New test groups observed: `touchpad-core` unit 143, public M8 integration 8, Linux M8 replay integration 2.
- Credential-pattern scan outside generated/cache directories: 0 files.
- Scope scan found only offline core/tests/fixture/docs changes; no live input/output, `/dev/input`, grab, Portal, libei, or `output-probe --emit` command was run.

## Repair scope

Repair R1–R4 and add the targeted tests only. Preserve M1–M7, M8's correct behavior, the public configuration unless a compatibility-safe correction is required, and all accepted-prefix/cleanup guarantees. Keep the work offline. Do not start M9 or run any live input/output command.

---

## Re-review 1 — 2026-08-17

Decision: **REJECTED — one test-construction repair remains; do not start M9**

The R1–R4 implementation repairs are structurally correct, and independent verification passes: formatting, clippy with warnings denied, all 647 workspace tests, and the exact R2 discontinuity/physical-release regression in both debug and release profiles. No credential patterns were found outside generated/cache directories, and the repair stayed inside the reported offline core/test/documentation scope.

### R5 — Medium: the missing-coordinates row does not preserve the requested pre-frame physical state

`simultaneous_physical_transitions_with_synthetic_cancellation_wire_invariant` claims to cover each cancellation cause × `(physical held pre-frame, frame physical state)`. Its `run_missing_coords` setup does not do that. After optionally setting physical left to `pre_phys`, it begins the locked continuation using `f(...)`; that helper always sets physical left to false. Consequently, when `pre_phys == true`, the physical release occurs on the setup frame, before the missing-coordinate cancellation frame. On the target frame:

- `(pre_phys=true, frame_left=false)` is actually a stable false physical source, not a simultaneous physical release plus synthetic cancellation;
- `(pre_phys=true, frame_left=true)` is actually a physical press plus synthetic cancellation, not an already-held physical source.

This leaves two requested missing-coordinate transition cases untested and makes the coverage statement in `DESIGN_V2.md` inaccurate. Repair the fixture so the locked-continuation setup frame preserves `pre_phys` (or otherwise assert the actual source state immediately before the cancellation frame). Keep the full four-combination table for missing coordinates, assert the exact expected button edge sequence and final source/aggregate states for every combination, and run the R2 table in both debug and release profiles. Update documentation/test counts only if they actually change. No production behavior change is currently requested; if a corrected regression exposes one, repair it narrowly and report it explicitly.

All work remains offline. Do not start M9 or run live input/output commands.

---

## Re-review 2 — 2026-08-17

Decision: **APPROVED — M8 complete; M9 may start**

R5 is closed. The missing-coordinates setup frame now explicitly preserves `pre_phys`, and an immediate pre-cancellation assertion proves that the physical source, synthetic lock, and aggregate are in the state claimed by each table row. All 12 combinations (three cancellation causes × two pre-frame physical states × two incoming physical states) assert the exact button-edge sequence, final physical/synthetic/aggregate states, wire equivalence, and cancelled lock phase. The corrected regression exposed no production defect, so the repair is test-only.

Independent final verification:

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- corrected R2 table test, debug profile: pass.
- corrected R2 table test, release profile: pass.
- `cargo test --workspace --locked`: pass, 647 tests, 0 failed.
- credential-pattern scan outside generated/cache directories: 0 files.
- final repair changed only the test section of `crates/touchpad-core/src/arbiter.rs`; no production behavior, live input/output, device grab, portal/libei call, or desktop emission occurred.

R1–R5 are closed. M8's offline tap, tap-and-drag, sticky drag-lock, source-aware left-button arbitration, checked timing, discontinuity handling, accepted-prefix delivery, and unconditional cleanup contracts are approved as the baseline for M9.
