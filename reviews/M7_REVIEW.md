# M7 Review — Offline Arbiter, One-Finger Pointer, Physical Click

Date: 2026-08-16  
Decision: **REJECTED — repair M7; do not start M8**

The unified lifecycle, candidate-period suppression, threshold/first-delta behavior, typed linear mapping, per-axis remainder, physical-button ordering, offline trace parity, and broad deterministic coverage are good. Independent fmt, clippy, and test gates all pass. Two failure-boundary defects must be repaired before this state machine is safe to wire into a later takeover runtime.

## Blocking findings

### R1 — Critical: `ArbiterSink` commits output state before acceptance and makes failed releases non-retryable

`ArbiterSink::frame` first calls `self.arbiter.frame(frame)`, which commits the complete decision, and only then submits its events one by one. A rejected `ButtonDown` is therefore recorded by the arbiter as held even though the sink did not accept it; the current test explicitly treats that mismatch as success and later emits an unmatched `ButtonUp`. A mid-batch failure also returns only the sink error, permits subsequent frames, and provides no accepted-prefix/faulted-state contract.

The shutdown path is more dangerous: `ArbiterSink::release_all` calls `self.arbiter.release_all()` first, which clears the held state, then submits the returned `ButtonUp`. If that submit fails, the next cleanup attempt produces no event, so an actually accepted down can remain stuck forever. The adapter also never calls the wrapped `OutputSink::release_all`, despite owning that sink and exposing a method with the same lifecycle name.

Repair the adapter around explicit delivery acknowledgement and fail-stop semantics. Required observable behavior:

- a rejected down is not treated as delivered and must not cause an unmatched up;
- an accepted down followed by a failed motion/up remains known as delivered-held until cleanup succeeds;
- any partial frame submission faults the adapter and blocks further normal frames; it cannot silently continue from an output state that diverged from the decision state;
- cleanup releases only state actually accepted by the sink, invokes the wrapped sink's cleanup contract as appropriate, and remains retryable after either explicit release submission or sink cleanup failure;
- arbiter/pointer state is reset only at a well-defined acknowledgement boundary; no failed cleanup can erase the fact that a release still needs retrying;
- structured errors preserve the failed event/index, accepted prefix, primary failure, and cleanup failure when both exist, without pretending the whole decision was delivered.

Add fault-injection tests for rejection at: first down; movement after an accepted down; up after an accepted down; first and repeated cleanup attempt; wrapped `OutputSink::release_all`; and successful recovery/reset followed by a fresh interaction. Assert exact accepted wire/event logs and prove no duplicate down, unmatched up, or permanently lost release.

### R2 — High: frame validation ignores existing Error/Fatal contact invariants

`Arbiter::frame` calls a private `structural_error` that checks only duplicate slots. It does not consume `ContactFrame::validate()`, so a live contact with a negative tracking id, non-finite/out-of-range pressure, non-finite orientation, or negative ellipse size can be accepted even though the core model classifies those diagnostics as Error and says the affected data must not be trusted. In particular, the arbiter can begin and commit an interaction whose tracking id is negative/reserved.

Use the existing model validation rather than duplicating a subset. Reject the frame atomically when `ContactFrame::validate()` produces any Error/Fatal diagnostic, with structured code/reason context and no state/button/baseline change. Warning-only cases retain their intended policy: an incomplete `Began` contact produces no candidate/output and a diagnostic, while a missing coordinate on the active tracked contact cancels it and still processes a physical release. Add tests for negative live tracking id, invalid pressure/orientation/ellipse, warning-only incomplete begin, and state atomicity when an invalid frame also changes the physical button bit.

## Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 554 tests, 0 failed.
- Targeted M7 tests: core arbiter unit/integration and Linux replay-derived parity all pass, but current sink-failure tests encode the R1 mismatch as the expected behavior.
- Credential-pattern scan outside generated/cache directories: 0 files.
- No live input/output, `/dev/input`, grab, Portal, or libei command was run for M7 review.

## Repair scope

Repair R1–R2 only. Preserve M1–M6 and M7's correct pure arbiter behavior. Keep all work offline; do not start M8, run `output-probe --emit`, or access/grab `/dev/input`.

---

## Re-review 1 — 2026-08-17

Decision: **REJECTED AGAIN — repair R3 only; do not start M8**

R2 is accepted: `Arbiter::frame` now consumes `ContactFrame::validate()`, rejects every Error/Fatal diagnostic atomically with structured codes/reason, preserves warning-only handling, and covers negative tracking ids, invalid pressure/orientation/ellipse, incomplete begins, and invalid-frame/button-edge atomicity.

Most of R1 is also repaired correctly: delivery is acknowledged event by event; partial submissions report the failed index, accepted prefix, failed event, and primary error; the adapter becomes fail-stop; rejected downs do not become held; accepted downs survive partial failure; and wrapped cleanup failures are preserved and retryable. One cleanup reconciliation branch remains incorrect.

### R3 — Critical: successful wrapped cleanup is not treated as authoritative

In `ArbiterSink::release_all`, if the explicit `ButtonUp(Left)` submission fails but the subsequent wrapped `OutputSink::release_all()` succeeds, `delivered_held_left` remains `true`. The next cleanup attempt therefore submits another explicit `ButtonUp`. That contradicts the `OutputSink` contract: a successful `release_all()` releases **all** held button/key state and is idempotent, so the adapter must reconcile to not-held after that acknowledgement. Against a real stateful sink this path can emit an unmatched/duplicate up on the next retry.

The current `ScriptedSink` only records accepted events; its `release_all()` does not model or clear held state. Consequently `cleanup_retries_after_failed_release_submission` encodes the wrong expectation for the explicit-failure/wrapped-success combination.

Repair only this cleanup acknowledgement matrix:

- explicit up fails + wrapped cleanup succeeds: preserve/report the explicit failure, but reconcile held state to released; a later recovery call must not submit another up;
- explicit up fails + wrapped cleanup fails: retain held state and retry the explicit up;
- explicit up succeeds + wrapped cleanup fails: retain not-held state and retry only wrapped cleanup;
- both succeed: reset normally;
- preserve both errors when both fail, and keep the adapter's fault/recovery behavior explicit and deterministic.

Replace or extend the fault-injecting sink with a real held-state model: accepted down sets held, accepted up clears held, and successful wrapped `release_all()` clears held. Assert exact submit attempts, accepted event log, cleanup-call count, adapter/arbiter state, and no second up after wrapped cleanup already succeeded. Cover both initially healthy and already-faulted adapters where applicable. Update the method documentation so it distinguishes explicit-up acknowledgement from the authoritative wrapped-cleanup acknowledgement.

### Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 570 tests, 0 failed.
- Credential-pattern scan outside generated/cache directories: 0 files.
- No live input/output, `/dev/input`, grab, Portal, or libei command was run.

Do not broaden the repair beyond R3 and its tests/docs. Do not start M8.

---

## Re-review 2 — 2026-08-17

Decision: **APPROVED — M7 complete; M8 may start**

R3 is accepted. `ArbiterSink::release_all` now treats a successful wrapped `OutputSink::release_all()` as the authoritative acknowledgement that all held state has been released, even when the preceding explicit `ButtonUp(Left)` submission failed. Delivery knowledge and arbiter held state are reconciled to not-held, the explicit error remains visible, and a later recovery call does not submit an unmatched or duplicate up.

The four cleanup quadrants are now deterministic and covered with stateful fault sinks at both unit and public-contract levels:

- explicit up fails + wrapped cleanup succeeds: explicit failure reported, held cleared, no second up;
- both fail: both errors preserved, held retained, explicit up retried;
- explicit up succeeds + wrapped cleanup fails: not-held retained, only wrapped cleanup retried;
- both succeed: full reset at the acknowledgement boundary.

Coverage also includes the authoritative-cleanup branch on an already faulted adapter: normal frames remain blocked until the recovery cleanup resets the adapter, without re-submitting the up. Tests assert accepted event logs, submission attempts, cleanup calls, sink-held state, arbiter-held state, lifecycle, and fault state.

### Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 574 tests, 0 failed.
- Credential-pattern scan outside generated/cache directories: 0 files.
- R3 scope is limited to `crates/touchpad-core/src/arbiter.rs` and `crates/touchpad-core/tests/m7_arbiter.rs`.
- No live input/output, `/dev/input`, grab, Portal, libei, or `output-probe --emit` command was run.

M7's review gate is closed. M8 may begin as a separate offline milestone.
