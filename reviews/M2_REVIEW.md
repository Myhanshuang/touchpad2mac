# M2 External Review — Versioned Trace and Offline Replay Boundary

Status: **APPROVED**  
Scope reviewed: `crates/touchpad-trace`, M2 fixtures/tests, workspace metadata, and the M2 sections of `DESIGN_V2.md`  
Review gate: M3 must not start until every required item below is fixed, independently rechecked, and this status is changed to **APPROVED**.

## Independent checks

- `cargo fmt --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass (110 tests: 87 unit, 21 integration, 2 doc tests)
- Secret scan outside `target`: no API key/token material found
- Scope scan: no M3 decoder, Linux device access, ioctl, grab, or CLI implementation found

The green suite is necessary but not sufficient: the failure injection and numeric-boundary cases below are currently untested.

## Required changes

### R1 — A partial event write can corrupt the stream, but the writer claims it remains usable

Severity: **high**

`TraceWriter::write_event` performs two fallible writes (JSON bytes and newline). A generic `Write::write_all` may return an error after writing a prefix. The implementation then returns the I/O error without changing state or advancing `next_line`; a retry can append another JSON object to that prefix and silently produce a corrupted line. This contradicts the public claim that a failed write leaves the writer usable and the module claim that each line is written in full.

Required resolution:

- Introduce an explicit poisoned/failed writer state after any event-line I/O failure for which partial output may have occurred.
- After poisoning, reject `write_event`, `flush`, and `finish` deterministically with structured state/error semantics; do not imply retry is safe.
- Keep validation/serialization failures that happen before any bytes are written recoverable if desired, but document that distinction precisely.
- Add a fault-injecting `Write` test that fails after a chosen byte count, proves a partial line is possible, and proves the writer cannot append/finish as though the trace were clean.
- Correct the statements in `writer.rs` and `DESIGN_V2.md` that currently over-promise full-line atomicity or post-I/O-error usability.

### R2 — The reader narrows declared integer fields before validation and misclassifies range errors as corrupted JSON

Severity: **high**

The schema declares `sec: u64`, but `RawEvent` first deserializes it as `i64`. Values in `i64::MAX + 1 ..= u64::MAX` therefore fail serde before the checked timestamp conversion and are reported as `CorruptedLine`, even though they are syntactically valid integer fields and should reach `InvalidField` (normally timestamp overflow). The same classification problem exists for out-of-range `type`, `code`, and `value`, which deserialize directly into `u16`/`i32`. `schema_version` is similarly narrowed to `i64`, so a positive integral version above `i64::MAX` cannot produce the promised explicit `SchemaTooNew` result.

Required resolution:

- Parse numeric JSON fields without premature narrowing (for example via `serde_json::Number` or equivalent raw numeric representation), then perform explicit signedness/integrality/range checks.
- Preserve the taxonomy: malformed/missing/wrong-shaped JSON is `CorruptedLine`; a present numeric field with invalid sign/range is `InvalidField`; a positive integral schema version representable in `u64` and newer than supported is `SchemaTooNew`.
- Ensure all documented `u64` `sec` values can be classified by field validation, including values above `i64::MAX`; conversion overflow must remain `InvalidField`, not `CorruptedLine`.
- Add boundary tests for `schema_version > i64::MAX`, `sec == i64::MAX + 1`, negative and overflowing `type`/`code`, and overflowing `value`. Also cover fractional/non-number numeric fields with an explicitly documented classification.

### R3 — Fatal reader errors are not terminal, so callers can bypass the header-first and time-validity state machine

Severity: **medium**

On a failed `read_header`, state remains `AwaitingHeader` even though line 1 was consumed. A caller can call `read_header` again and accept a header on line 2, violating the public “first line must be the header” invariant. Likewise, after a corrupted event, time regression, or I/O error, direct `read_event` calls continue from later lines. `ReplayDriver` and `Events` stop on the first error, but the public `TraceReader` itself does not enforce the stated terminal failure contract.

Required resolution:

- Add a terminal failed/poisoned reader state and enter it whenever consuming/parsing/validating a trace line fails, including underlying I/O failure.
- Subsequent header/event operations must fail deterministically rather than resume after the offending line. Clearly document the returned state error.
- Add tests proving a second-line header cannot be accepted after line 1 failed and that a later event cannot be consumed after a time regression/corrupted line.

### R4 — The intentional writer/reader timestamp asymmetry contradicts the round-trip contract and lacks an end-to-end test

Severity: **medium**

The chosen policy can be valid: recording preserves a regressed kernel timestamp, while replay diagnoses `TimeRegression`. However, `writer.rs` says the crate can never produce a trace its own reader rejects, and `tests/roundtrip.rs` says whatever the writer emits must read back exactly. Both are false under the documented recording-fidelity policy.

Required resolution:

- Keep or change the policy deliberately; do not silently normalize/drop raw timestamps.
- If keeping the current policy, explicitly define a regressed capture as a faithfully recorded but replay-invalid diagnostic artifact, remove the universal round-trip claims, and add an end-to-end test: writer accepts and preserves the regression; reader/replay returns `TimeRegression` at the correct line.
- Ensure `DESIGN_V2.md` states this exception and does not claim all successful writer output is replay-accepted.

## Re-review requirements

The fix pass must remain strictly within M2. Re-run all three workspace gates and report exact test counts. Do not begin M3 or add decoder/device/CLI code. External review will inspect the resulting diff and independently execute the gates before approval.

## Re-review result

Re-reviewed after the dsh R1–R4 fix pass on 2026-08-15. All required changes are closed:

- **R1 closed:** event-line I/O failures poison the writer; validation failures remain recoverable; byte-limited fault injection covers a mid-JSON failure and a terminator failure; poisoned `write_event`/`flush`/`finish` are rejected.
- **R2 closed:** raw JSON numbers are explicitly classified before narrowing. Boundary coverage includes schema versions above `i64::MAX`, `sec` through `u64::MAX`, signed/range failures for every event integer, fractional/exponent values, non-number values, and missing fields.
- **R3 closed:** line-consuming reader failures are fail-stop. Header, corrupt-line, time-regression, and underlying-I/O tests prove later lines cannot be resumed; non-consuming API misuse remains recoverable.
- **R4 closed:** timestamp regression remains faithfully recorded but replay-invalid by deliberate policy; documentation no longer promises universal writer→reader acceptance, and the end-to-end test checks exact line-3 diagnosis plus no sink `finish`.

Independent re-run:

- `cargo fmt --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo test --workspace`: pass, **134 passed / 0 failed** (110 unit, 22 integration, 2 doc tests)
- `cargo metadata --no-deps --format-version 1`: pass; workspace still contains only `touchpad-core` and `touchpad-trace`
- Secret scan outside `target`: no matches
- Scope scan: no M3 decoder, Linux device/ioctl/grab, or CLI implementation

Decision: **M2 is approved.** M3 may begin only as a separate milestone run under the existing external-review gate.
