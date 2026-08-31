# M3 External Review — Type-B Decoder and Mocked Resynchronization

Status: **APPROVED**  
Scope reviewed: `crates/touchpad-linux`, M3 fixture changes, workspace metadata, and M3 sections of `DESIGN_V2.md`  
Review gate: M4 must not start until every required item below is fixed, independently rechecked, and this status is changed to **APPROVED**.

## Independent checks

- `cargo fmt --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass (169 tests: 136 unit, 31 integration, 2 doc tests)
- Secret scan outside `target`: no API key/token material found
- Scope scan: no real `/dev/input`, ioctl/FFI, grab, syscall adapter, signal, or CLI implementation found

The suite is green, but several tests currently encode unsafe decoder behavior as expected behavior. M3 is not approved until the state machine fails closed on the cases below.

## Required changes

### R1 — An invalid slot selection lets later fields corrupt the previously selected slot

Severity: **high**

`on_abs` reports an out-of-range `ABS_MT_SLOT` but deliberately keeps `current_slot` unchanged. Subsequent tracking/position events are therefore applied to the previous valid slot even though they belonged to an invalid/unknown selection. The existing test explicitly expects slot 0 to be modified after selecting slot 99. A malformed stream can silently corrupt a real contact while carrying only a diagnostic.

Required resolution:

- After an invalid `ABS_MT_SLOT`, mark slot selection invalid and ignore all slot-scoped `ABS_MT_*` events until a valid slot is selected; do not redirect them to the previous slot.
- Preserve the protocol default that slot 0 is selected initially, if desired, but invalid selection must revoke it.
- Emit structured diagnostics without panic and add tests proving the old slot remains unchanged, ignored fields do not leak, and a later valid selection resumes normally.

### R2 — Tracking-id handling does not implement the exact `-1` rule and loses lifecycle event order

Severity: **high**

The implementation treats every negative tracking id as an end, while the contract says exactly `ABS_MT_TRACKING_ID == -1` ends a contact. In addition, `PendingSlot` stores only `tracking_begin: Option<i32>` plus `tracking_end_seen: bool`; this collapses order. `end(-1) -> begin(new)` and `begin(new) -> end(-1)` in one SYN cycle become the same tuple, although the former should leave a new lifecycle at the boundary and the latter should leave no live lifecycle. Multiple tracking transitions are also reduced to the last id plus one boolean.

Required resolution:

- Accept only `-1` as the end sentinel. Values `< -1` must be diagnosed and ignored; they must not end or replace a contact.
- Preserve the order of tracking transitions within a SYN cycle (an explicit transition list/state machine is acceptable). Define deterministic semantics for end→begin, begin→end, direct id replacement, and repeated ids without publishing half frames.
- Add tests for `< -1`, end→begin, begin→end, repeated id, and multiple replacement transitions. No prior-contact fields may leak into a new tracking lifecycle.

### R3 — Touch major/minor are lengths but are normalized with absolute-position origin semantics

Severity: **high**

`ABS_MT_TOUCH_MAJOR` and `ABS_MT_TOUCH_MINOR` are contact dimensions (deltas/lengths), yet both pending and snapshot paths call the absolute position conversion, which subtracts `AxisInfo.min`. M1 deliberately split position conversion from delta conversion to prevent this exact origin error. A non-zero axis minimum produces the wrong physical contact size.

Required resolution:

- Use the core delta/length conversion for touch major/minor, while keeping absolute position conversion for X/Y.
- Preserve resolution/profile override and missing-resolution diagnostics.
- Add pending and snapshot/resync tests with a non-zero axis minimum that distinguish position from delta semantics.

### R4 — Invalid or incomplete resync snapshots are published as trusted recovery frames

Severity: **high**

`apply_snapshot` mutates live decoder state directly, skips out-of-range slots with a diagnostic, accepts duplicate slots, and publishes active slots even when required X/Y data is missing. The existing out-of-range test expects a successful `discontinuity = true` frame. This contradicts the `ResyncSource` complete/internally-consistent snapshot contract and the fail-safe rule: recovery failure must become `Degraded`, with no trusted output. The mutation is also not truly atomic because validation and construction do not complete before replacing live state.

Required resolution:

- Validate a snapshot completely before changing committed/pending/button/current-slot state.
- At minimum reject active out-of-range slots, duplicate slot entries, invalid tracking ids, and active contacts missing required raw X or Y. Validate any additional descriptor-required fields the decoder relies on.
- Build a complete draft state and swap it in only after validation/normalization succeeds. An invalid snapshot is a resync failure: enter `Degraded`, return a structured fatal error, and publish no discontinuity frame.
- Replace the current “out-of-range snapshot succeeds with diagnostic” test and add duplicate/incomplete/invalid-id tests plus proof that no frame is emitted and later feeds remain degraded.

### R5 — Replay reports success when a trace ends with unresolved `SYN_DROPPED`

Severity: **medium**

The decoder's `ReplaySink::finish` always returns `Ok(())`. A trace that ends after `SYN_DROPPED` but before the recovery `SYN_REPORT` therefore returns successful replay statistics while the decoder remains `DroppedAwaitingBoundary` and continuity was never restored.

Required resolution:

- On replay finish, require a trustworthy terminal synchronization state. An unresolved dropped/recovering/degraded state must return a structured decode/replay error and must not emit a frame.
- Add an end-to-end trace/replay test that ends after `SYN_DROPPED` and proves replay fails rather than reporting clean completion.
- Document the distinction between an ordinary trace ending between frames and a trace ending with unresolved loss of synchronization.

### R6 — Untrusted `slot_count` can request an effectively unbounded allocation

Severity: **medium**

`configure` accepts any non-zero `u32` slot count and immediately creates two vectors of that length. A replay-controlled header can therefore request billions of slots and cause allocation failure/process abort instead of a structured `InvalidDevice` error. This violates the no-panic/fail-safe boundary expected for offline replay.

Required resolution:

- Define and document a defensible maximum supported Type-B slot count and/or use a fallible allocation path; reject unreasonable/unallocatable descriptors with `DecodeError::InvalidDevice` before constructing decoder state.
- Add boundary tests for the maximum accepted value and the first rejected/unreasonable value without performing a huge allocation.

## Re-review requirements

The fix pass must remain strictly within M3. Update `DESIGN_V2.md` to match actual corrected semantics. Re-run all three workspace gates and report exact test counts. Do not begin M4/M5 or add device access, ioctl/FFI, grab, syscall, signal, or CLI code. External review will inspect the resulting implementation and independently execute the gates before approval.

## Re-review 1 — R2 remains open

The first fix pass closes R1 and R3–R6, but **does not fully close R2**. `tracking_transitions` preserves only the order of tracking-id events; the pending X/Y/pressure/shape fields are still one last-value bucket for the entire SYN cycle. Their position relative to lifecycle transitions is lost.

Concrete failures still possible:

- Existing contact: `X(old update) -> TRACKING_ID(new) -> Y(new update) -> SYN_REPORT`. Commit resets to the new lifecycle and applies the cycle-wide X and Y bucket, so the old lifecycle's X incorrectly completes the new contact.
- Existing contact: `TRACKING_ID(-1) -> X(after end) -> SYN_REPORT`. The post-end X is retained and applied to the `Ended` contact even though no lifecycle was active when that field arrived.
- Longer begin/end/replacement chains have the same cross-lifecycle field-association problem. The ordered tracking-id vector alone is therefore not an ordered lifecycle state machine.
- `Vec<TrackingTransition>` also grows without a bound until `SYN_REPORT`; a replay-controlled stream can send arbitrarily many tracking-id events without a boundary and consume unbounded memory.

Required follow-up:

- Associate field updates with the lifecycle that is active **at event arrival time**, not merely with the final tracking-id result at commit. Processing tracking transitions incrementally into bounded per-slot pending state is preferred.
- A real `Begin(new_id)` must start a clean pending lifecycle and discard fields belonging to the previous lifecycle; a repeated begin of the same effective id must not reset it.
- A field arriving when the effective lifecycle is ended/empty must be diagnosed and ignored. It must not alter a prior `Ended` contact or a later `Began` contact.
- Preserve the already-correct exact `-1`, end→begin, begin→end, replacement, inherited-field, and SYN_REPORT-only publication semantics.
- Eliminate the unbounded transition vector or impose a defensible constant-memory state representation.
- Add regression tests for field-before-replacement leakage, field-after-end leakage, and interleaved field updates across multiple replacements. Tests must prove an incomplete new contact remains held when only one coordinate arrived after its begin.

Independent gates after fix pass 1 remain green (`cargo fmt --check`, strict workspace Clippy, 184/184 tests), but green tests do not close this semantic gap. Status remains **CHANGES REQUESTED** and M4 remains blocked.

## Final re-review result

Re-reviewed after the focused R2 lifecycle/field-order follow-up on 2026-08-16. All required items are now closed:

- **R1 closed:** invalid slot selection revokes the selection; later slot-scoped fields are diagnosed and ignored until a valid slot is selected, never redirected to the old slot.
- **R2 closed after re-review:** `PendingLifecycle` is updated at event arrival time; fields are bound to the then-active lifecycle, cleared on real replacement, ignored after end/while empty, and stored in bounded per-slot state. Regression tests cover old-field→replacement leakage, post-end leakage, multiple interleaved replacements, incomplete new contacts, and bounded replacement diagnostics.
- **R3 closed:** touch major/minor use delta/length conversion in normal and snapshot paths; non-zero-min tests distinguish it from absolute position conversion.
- **R4 closed:** snapshots are fully validated and built as a draft before atomic swap; invalid/out-of-range/duplicate/incomplete snapshots degrade with no frame and reject later feeds.
- **R5 closed:** replay finish rejects unresolved synchronization loss and accepts ordinary Normal-state completion.
- **R6 closed:** documented `MAX_SLOT_COUNT = 256`; 256 is accepted, 257 and replay-controlled unreasonable counts are rejected before allocation.

Independent final checks:

- `cargo fmt --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass, **190 passed / 0 failed** (154 unit, 34 integration, 2 doc tests)
- Secret scan outside `target`: no matches
- Scope scan: no M4/M5 implementation; only documentation mentions `/dev/input`, ioctl, grab, signals, and CLI as absent/future work
- `touchpad-linux` remains `#![forbid(unsafe_code)]` with no platform/system dependency

Decision: **M3 is approved.** M4 may begin only as a separate milestone run under the existing external-review gate.
