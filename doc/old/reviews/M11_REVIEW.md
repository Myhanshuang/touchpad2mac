# M11 Review — Experimental Pointer Fidelity

Date: 2026-08-22  
Decision: **REJECTED — M11 core is substantially implemented, but M11 is not code-complete yet**

The current implementation has made major progress. The pure fidelity stage exists, `M11Profile` exists, the Arbiter owns fidelity state atomically, the fidelity-disabled M10 path remains available, M11 pointer routing is integrated through one narrow helper, and `crates/touchpad-core/tests/m11_fidelity.rs` now contains broad pure/core/lifecycle coverage. Independent debug and release workspace test suites both pass.

However, the milestone does **not** satisfy `M11_TASK.md` §13–§14 yet. Two mandatory quality gates fail, CLI verification is incomplete, trace/replay coverage is absent, acceptance/status documentation is absent or stale, and one public constructor still has a panic path inconsistent with its own `Result` contract. M11 must remain `live-unqualified`; do not run live takeover and do not begin M12.

## Blocking findings

### R1 — Critical: Batch 3 is incomplete — no M11 trace/replay fixture and no M11 acceptance/status documentation

`M11_TASK.md` §12 requires direct synthetic frames and trace/replay frames to produce identical M11 decisions. The current workspace has no M11 fixture under `crates/touchpad-trace/tests/fixtures/`, and a workspace search finds no M11 replay coverage outside the core M11 test file.

The required future acceptance document `docs/M11_ACCEPTANCE.md` does not exist. `README.md`, `DESIGN_V2.md`, and `MILESTONES.md` have not been updated for the current M11 implementation state; in particular, existing milestone/design text still says that M11 has not started. That is now materially stale and contradicts the workspace.

Required repair:

- add a deterministic M11 trace fixture covering first commit, low/high-speed movement, duplicate timestamps, reversal, diagonal motion, exact/over-long-gap behavior, clean end, and a fresh interaction;
- add replay tests proving direct synthetic frames and replay-derived frames produce identical M11 decisions;
- create `docs/M11_ACCEPTANCE.md` as a **future user-run procedure only** and do not execute it;
- update README/design/milestones so they clearly distinguish:
  - M10 code approval from pending M10 live qualification;
  - M10 acceptance still using `m10-linear-v1`, M6 calibration evidence, and ordered 10/60/300-second runs;
  - M11 as experimental, opt-in, provisional and live-unqualified;
  - M11 code completion not implying live qualification;
  - a separate future M11-specific acceptance;
  - no macOS equivalence claim;
  - M12 not begun.

### R2 — High: required CLI pure-helper/banner and fake command coverage is incomplete

The implementation now has a good pure `select_profile` helper in `apps/touchpadctl/src/cmd/takeover.rs`, and the parser accepts exactly `m10-linear-v1` / `m11-fidelity-v1`. The M11 banner text also contains the required experimental/uncalibrated/non-default/no-macOS/no-live-validation language and is emitted before device/output/recorder/countdown/grab work.

But the required automated coverage is incomplete:

- `apps/touchpadctl/src/cmd/takeover/tests.rs` currently has no tests calling `select_profile`;
- `apps/touchpadctl/tests/cli.rs` currently has no M11 CLI coverage;
- the previous DSH execution was interrupted exactly at the point where it said it would add pure-helper and M11 banner tests.

`M11_TASK.md` §11–§12 explicitly requires tests for pure profile routing/banner and fake-backed command paths.

Required repair:

- test `select_profile("m10-linear-v1")` constructs exactly M10 config and keeps fidelity disabled;
- test `select_profile("m11-fidelity-v1")` constructs exactly M11 config and enables fidelity;
- assert the M11 banner contains all five required claims;
- assert unknown profile fails in the pure helper without side effects;
- add fake-backed command/CLI coverage for M11 while preserving all five mandatory M10 takeover opt-ins, duplicate rejection, missing profile behavior and 1..=300 duration limits;
- do not enter a real takeover session.

### R3 — High: mandatory quality gates fail (`fmt` and `clippy`)

Independent verification:

- `cargo fmt --all -- --check`: **FAIL**. There are formatting diffs in new/modified M11 files including `apps/touchpadctl/src/args.rs`, `apps/touchpadctl/src/cmd/takeover.rs`, `crates/touchpad-core/src/fidelity.rs`, `crates/touchpad-core/src/lib.rs`, `crates/touchpad-core/src/m11.rs`, and `crates/touchpad-core/tests/m11_fidelity.rs`.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: **FAIL**. `clippy::too_many_arguments` is emitted for the newly-expanded `ArbiterState::commit` and `ArbiterState::commit_pointer` functions in `crates/touchpad-core/src/arbiter.rs`.
- `cargo test --workspace --locked`: **PASS**, 0 failed.
- `cargo test --release --workspace --locked`: **PASS**, 0 failed.

Because §13 requires all four gates to pass, M11 cannot be code-complete while `fmt` or `clippy` fails.

Required repair:

- run `cargo fmt --all` and commit only the formatter's deterministic changes;
- refactor the M11 pointer-call plumbing so `commit` / `commit_pointer` satisfy clippy without hiding the warning with a blanket allow unless there is a compelling documented reason. Prefer a small context struct/helper that groups timestamp/sequence/fidelity/output routing inputs while keeping the M10-disabled branch unchanged;
- rerun all four mandatory gates after all later Batch 3 changes are complete.

### R4 — Medium: `M11Profile::new()` can panic instead of returning its documented `Fidelity` error

`M11ProfileError` defines a `Fidelity(FidelityConfigError)` variant, and `M11Profile::new()` publicly returns `Result<Self, M11ProfileError>`. But the current constructor builds the fidelity configuration with:

```text
FidelityConfig::new(...).expect("documented M11 constants validate")
```

If a future edit makes one documented M11 constant invalid, the public constructor panics and the `Fidelity` error variant is unreachable. That is inconsistent with the constructor's own error contract and makes profile validation less robust than intended.

Required repair:

- propagate `FidelityConfig::new(...)` with `.map_err(M11ProfileError::Fidelity)?` (or an equivalent non-panicking propagation);
- add a focused unit-level guard if needed so the error variant is not dead API; do not duplicate M7–M9 constants and do not edit `m10.rs`.

## Positive findings

The following M11 work is directionally correct and should be preserved during repair:

- `FidelityState` is stored inside `ArbiterState`, so it participates in the existing frame draft/copy/commit atomicity model.
- New interactions reset fidelity state together with the existing pointer remainder.
- `emit_pointer_delta` provides a narrow M10-disabled / M11-enabled switch.
- Fidelity-disabled operation stays on the existing linear quantization path without calling M11 fidelity code.
- M11 `Hold` / `Reanchored` paths emit no pointer movement and preserve pixel remainder as required.
- M11 scaled emission uses the existing truncation-toward-zero per-axis remainder; no second pixel remainder was introduced.
- Clean end processes final committed movement before interaction reset.
- Tracking-id replacement processes the old committed final movement before finishing/resetting and beginning the new candidate.
- Discontinuity cancels before contact handling.
- Second-contact cancellation discards one-finger fidelity state.
- The current M11 integration test file contains 57 tests spanning configuration validation, first-call semantics, duplicate timestamps, long-gap boundaries, signed cancellation/reversal/diagonal behavior, gain/scalar properties, 60/120 Hz comparison, profile inheritance, M10-disabled regression, Arbiter remainder/rollback/lifecycle behavior, tap/drag/lock, two-finger and physical-button ownership.
- `apps/touchpadctl/src/args.rs` now names the accepted profile set exactly as `{m10-linear-v1, m11-fidelity-v1}` and keeps M10 mention-first.
- `apps/touchpadctl/src/cmd/takeover.rs` has a pure profile selector and an explicit M11 experimental banner before device/output/recorder/countdown/grab work.
- `crates/touchpad-core/src/m10.rs` has not been modified by the M11 implementation.
- `#![forbid(unsafe_code)]` remains in `touchpad-core`, and no new `unsafe` occurrence is present in the inspected M11 core files.
- Workspace credential scan excluding `.env`, generated targets and `.git` found no DeepSeek key/base-url/provider endpoint text or obvious `sk-...` token material.
- No live device/grab/portal/libei/output/system-setting command was executed during this review.

## DSH repair plan — split into ~5 minute parts

Do not give DSH the whole repair at once. Execute these parts sequentially, reviewing each part's actual file changes and targeted tests before starting the next.

### Part 1 — core cleanup and mandatory static gates

Scope:

- run `cargo fmt --all`;
- repair the two `clippy::too_many_arguments` failures with a small structured pointer-routing context/refactor;
- replace the `M11Profile::new()` fidelity `.expect(...)` with non-panicking error propagation;
- run:
  - `cargo fmt --all -- --check`;
  - `cargo clippy -p touchpad-core --all-targets --locked -- -D warnings`;
  - `cargo test -p touchpad-core --test m11_fidelity`.

Stop after those pass. No CLI/replay/docs work in this part.

### Part 2 — CLI pure helper and fake-backed M11 tests

Scope:

- add `select_profile` tests;
- add M11 banner assertions;
- add fake-backed/public CLI M11 profile tests while preserving all five takeover opt-ins and duration bounds;
- run focused touchpadctl tests plus fmt/clippy for touchpadctl.

Stop after focused CLI tests pass. No replay/docs work in this part.

### Part 3 — M11 trace/replay fixture

Scope:

- add deterministic M11 trace fixture;
- add direct-vs-replay decision equality tests covering the required timing/motion lifecycle cases;
- run focused trace/linux replay tests plus relevant core test target.

Stop after replay-focused tests pass. No docs work in this part.

### Part 4 — M11 acceptance and status documentation

Scope:

- create `docs/M11_ACCEPTANCE.md` as a future user-run procedure, not executed;
- update README, DESIGN_V2 and MILESTONES with accurate M10/M11 qualification boundaries;
- ensure no review-pass or live-qualified claim is made;
- ensure M12 remains not begun.

Stop after documentation consistency review. Do not run live commands.

### Part 5 — final offline gates and handoff evidence

Run exactly:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

Then verify/report:

- no new dependency;
- no new unsafe code;
- no credentials in changed non-generated text;
- no live command ran;
- `m10.rs` unchanged;
- M10/output still live-unqualified;
- M11 still live-unqualified pending separate future user acceptance;
- M12 not begun.

Only after all five parts are complete should this review receive a re-review. Do not self-approve M11 in DSH output.

## Review conclusion

M11's core fidelity/Arbiter implementation is now substantial and the broad debug/release test suites pass, so the milestone is much closer to completion than the previous partial state. The remaining work is bounded and well-defined, but it includes mandatory contract items and failing required gates. Therefore M11 is **not code-complete and not approved** at this review point.

Keep all repair work offline/fake-backed. Do not run `touchpadctl takeover`, do not perform real output/device tests, and do not begin M12.

---

## Re-review 1 — 2026-08-22

Decision: **APPROVED — M11 code-complete; ready for separate future user live acceptance; remain live-unqualified**

R1–R4 are closed.

- **R1 closed — Batch 3 completion / replay / docs.** `crates/touchpad-trace/tests/fixtures/m11_fidelity.jsonl` now provides a deterministic 25-frame M11 scenario covering first commit, low/high-speed movement, duplicate timestamps, reversal, diagonal movement, exact/over-long-gap behavior, clean end and a fresh interaction. `crates/touchpad-linux/tests/m11_arbiter.rs` replays the fixture through the real `ReplayDriver` + `TypeBDecoder`, builds equivalent direct synthetic frames, and proves frame content and per-frame M11 `FrameDecision`s are identical. `docs/M11_ACCEPTANCE.md` now exists as a future user-run procedure and has not been executed. README/DESIGN/MILESTONES status text is no longer stale.
- **R2 closed — CLI pure helper/banner/fake path coverage.** `select_profile` now has direct tests proving `m10-linear-v1` constructs exactly M10 config with fidelity disabled and `m11-fidelity-v1` constructs exactly M11 config with fidelity enabled. The M11 banner tests cover experimental, uncalibrated, non-default, no-macOS-equivalence, no-live-validation, retained M10 opt-ins and the 1..=300-second bound. Fake-backed public CLI/takeover tests cover M11 routing, mandatory opt-ins, duplicate/missing profile errors and duration bounds without entering a real takeover.
- **R3 closed — formatting and clippy.** The pointer-routing call plumbing was grouped into a small `PointerRouting` context rather than suppressing `clippy::too_many_arguments`; formatter output is clean and no blanket allow was added.
- **R4 closed — non-panicking M11 profile construction.** `M11Profile::new()` now propagates `FidelityConfig::new(...)` through `M11ProfileError::Fidelity` instead of calling `expect`.

### Independent final verification

Run against the final Parts 1–4 workspace after documentation updates:

- `cargo fmt --all -- --check`: **PASS**.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: **PASS**, 0 warnings.
- `cargo test --workspace --locked`: **PASS**, 0 failed.
- `cargo test --release --workspace --locked`: **PASS**, 0 failed.
- No Cargo manifest or `Cargo.lock` changed during M11; no new dependency was added.
- `touchpad-core` still has `#![forbid(unsafe_code)]`; no new unsafe was added by M11. The two `unsafe { libc::raise(...) }` sites found in `apps/touchpadctl/tests/cli.rs` are the pre-existing M6 real-signal integration tests, not M11 code.
- Credential-pattern scan of M11-modified source/docs/tests: 0 matches.
- `crates/touchpad-core/src/m10.rs` remains unchanged from 2026-08-17.
- No M12 artifact exists and M12 has not begun.
- No live `/dev/input` open/grab, real portal/libei session, desktop input emission, system-setting mutation, or `touchpadctl takeover` command was executed during M11 implementation/review.

### Approval boundary

M11 is **approved as code** and may now be described as **code-complete**. This approval is strictly offline/fake-backed and does **not** confer live qualification.

M10 remains code-approved but `live-unqualified / pending user acceptance` until the user records the M6 calibration evidence and completes the ordered 10-second, 60-second and 300-second `m10-linear-v1` acceptance in `docs/M10_ACCEPTANCE.md`.

M11 remains separately **experimental / provisional / live-unqualified** until the user later runs and records the distinct procedure in `docs/M11_ACCEPTANCE.md`. M10 acceptance does not qualify M11. `m11-fidelity-v1` remains opt-in only, never the default, and makes no macOS-equivalence claim. This review does not approve M12, daemon/autostart behavior, unbounded takeover, or any production-default switch.
