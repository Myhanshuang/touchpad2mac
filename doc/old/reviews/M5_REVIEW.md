# M5 Review — CLI Vertical Slice and Phase 1 Handoff

Status: **APPROVED**

Reviewed: 2026-08-16 (Asia/Shanghai)

Scope: M5 only. M1–M4 remain approved. No post-M5 gesture, output-backend, daemon, or desktop-policy work is authorized by this review.

Latest decision (final re-review plus current-machine smoke): **APPROVED**.
R1–R5 are closed. The earlier findings and repair requests remain below as the
review history; the final acceptance section at the end is authoritative.

## Independent verification

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace`: pass, **349 tests total** (347 normal tests plus 2 doc tests), 0 failed.
- `cargo run -q -p touchpadctl -- --help`: pass, exit 0; the help contains the required default-off exclusivity warning.
- `cargo run -q -p touchpadctl -- replay crates/touchpad-trace/tests/fixtures/single_contact.jsonl`: pass, exit 0; three JSON `ContactFrame` lines and a summary.
- Credential scan outside build artifacts: 0 matches.
- Environment observation only: x86_64, rustc/cargo 1.97.1, `/dev/input` absent. No real device was opened or grabbed and no hardware behavior is approved.
- Negative CLI check: `touchpadctl replay ... --grab` incorrectly succeeds with exit 0, confirming R5 below.

The green gates establish build/test health, but they do not close the following lifecycle and safety-contract defects.

## Blocking findings

### R1 — The safe signal-handler API can leave `FLAG_PTR` dangling and does not enforce exclusive installation

`sys::ffi::install_termination_handler(&Arc<AtomicBool>)` stores `Arc::as_ptr(flag)` in the process-global `FLAG_PTR`, but `sys::ffi::TerminationHandlerGuard` owns only the previous `sigaction` values and a boolean. The public wrapper guard in `signals.rs` likewise does not retain an `Arc` clone.

The function is safe, yet a safe caller can drop the last `Arc<AtomicBool>` while the guard remains alive and then receive a signal; `termination_handler` dereferences freed memory. A documentation sentence saying that the flag “must outlive” the guard is not sufficient for a safe Rust API. The same safe API also permits two simultaneous installs even though it documents “only one”: the second overwrites `FLAG_PTR`, and dropping guards out of order can restore the wrong disposition, clear the active pointer, or leave the custom handler installed with a null/stale target.

Required:

- Make the guard own an `Arc<AtomicBool>` clone for the full installed lifetime; do not rely on caller-enforced lifetime prose for memory safety.
- Enforce the single-active-install invariant in code and return a structured error for a second install, or implement a demonstrably correct nesting model. A global raw pointer plus unenforced convention is not acceptable.
- Make restoration/clearing order safe against the supported process/thread model and document the exact concurrency boundary honestly.
- Add regressions proving that dropping the caller's `Arc` leaves the handler target alive through the guard, a second concurrent install is rejected (or correctly nested), and a fresh install succeeds after the first guard is dropped.

### R2 — `record --grab` grabs before the recorder/output is prepared

`cmd::record::run` calls `EvdevRuntime::open(... with_grab(grab) ...)` before it obtains the descriptor and calls `TraceRecorder::create`. Therefore `--grab` can issue `EVIOCGRAB(1)` before the output file is created, the header is accepted, or the recorder is attached. An unwritable output path can transiently take exclusive control of the touchpad and then rely on best-effort Drop cleanup.

This violates the project startup contract: open/validate and prepare recorder/decoder first; optional grab must be the last successful preparation step immediately before reading. It also makes the M5 claim “recorder prepared before exclusive operation” untrue. `TraceRecorder` wraps a `BufWriter`, so merely constructing it does not prove that the header reached the underlying file; preparation should include a successful flush before grab if the design claims the output is writable.

Required:

- Open and validate the runtime without grabbing, construct and attach the recorder from that same validated descriptor, successfully flush the header, and only then perform the explicit optional grab immediately before the read loop.
- Keep all operations on the same M4 session fd and preserve M4's at-most-once release behavior.
- Add one shared-timeline test proving header/recorder readiness precedes `EVIOCGRAB(1)`, plus a failure test proving an unwritable/header-flush-failing output causes **zero** grab calls.
- Correct comments/docs that currently describe the opposite order.

### R3 — Recorder finalization and cleanup failures are reported as successful controlled cleanup

The record command calls `runtime.shutdown()` first; that method flushes the recorder and then ungrabs/closes. Only **after** ungrab and close does the command remove the recorder and call `finish()`, although the module/design text says flush/finish occur before release. More importantly, `finish_result` is only printed, and `ShutdownReport.recorder_flush`, `ungrab`, and `close` failures do not affect the returned `CommandFailure`. A signal path always returns `CommandFailure::Stopped`, whose public message and help contract assert “trace flushed and device released,” even if flush/finish failed or close failed.

Fatal `fail_open` similarly discards cleanup errors. Preserving the primary stream/decoder error is reasonable, but cleanup failures must remain structurally observable and must never be converted into a false successful-cleanup assertion.

Required:

- Use one ordered finalization path: stop work → output-lifecycle no-op → recorder finish/flush → ungrab once → close regardless of prior errors. Do not call the potentially failing final `finish()` after release while documenting it as before release.
- Return a truthful structured/composite result that preserves both the primary stop/error reason and cleanup failures. Exit 8 may be used only when the trace finalization and device-release guarantees stated for exit 8 actually succeeded; otherwise return the appropriate recorder/cleanup failure.
- Define precedence for combined failures without losing either diagnostic.
- Add fault-injection tests for recorder flush failure, recorder finish failure, failed ungrab with successful close, close failure, and a primary decoder/stream failure combined with cleanup failure. Assert order and the final exit/message.

### R4 — Runtime fallback destruction can flush the recorder after ungrab/close

`EvdevRuntime` has no `Drop` implementation. Its `device` field is declared before `recorder`, so ordinary field destruction drops `DeviceHandle` (best-effort ungrab/close) before the recorder's writer performs its best-effort flush. This fallback is reachable through early `?` returns after recorder attachment—for example, failure to write the “recording …” status line—and through any unexpected unwind.

That contradicts the claim that every error path uses recorder flush before ungrab/close. The explicit `shutdown()` tests do not cover this fallback.

Required:

- Give the runtime an ordered best-effort Drop path, or restructure ownership so recorder finalization/flush is guaranteed to precede device release on fallback destruction. Explicit fallible shutdown remains the primary path.
- Add a test with a failing status/output writer after recorder attachment and a shared recorder/syscall timeline; prove flush precedes ungrab and close and each device operation occurs at most once.

## Major finding

### R5 — `--grab` is silently accepted by commands other than `record`

`parse_args` accepts `--grab` as a globally known flag, then `devices`, `inspect`, and `replay` ignore it. This contradicts both the parser documentation and help text (“record only”). Independent reproduction: `touchpadctl replay <valid fixture> --grab` exits 0 and performs the replay instead of returning usage exit 1.

Required:

- Reject `--grab` for every command except `record`; decide and document whether duplicate `--grab` is rejected.
- Add parser and actual-binary regressions for `devices --grab`, `inspect DEVICE --grab`, and `replay INPUT --grab`, all requiring usage exit 1 and no command execution.

## Documentation corrections

- `TraceRecorder::create`/DESIGN currently imply the header is immediately present in the file, but it is written into a `BufWriter`; either flush it during preparation or state the buffering contract accurately.
- `docs/M5_ACCEPTANCE.md` says every automated test runs through `MockSys`, while Linux FFI tests intentionally exercise real `sigaction`, `raise`, filesystem directory reads, and an open attempt on a nonexistent path. Keep the important claim (“no real device is successfully opened or grabbed”) but describe the non-mock side-effect-free tests accurately.
- Do not claim Rust 1.85 was tested: this review ran on 1.97.1. The declared MSRV may remain 1.85, but an actual 1.85 gate is needed before calling it independently verified.

## Approval gate

M5 is not approved. Fix R1–R5 within M5, correct the documents, add the fault-injection/negative tests above, rerun all quality gates and CLI smoke tests, and stop for another independent review. Do not begin post-M5 gesture or output-backend work.

## Post-repair re-review — 2026-08-16

### Independent verification

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace`: pass, **367 tests total** (365 normal tests plus 2 doc tests), 0 failed.
- Actual binary `--help`: exit 0 with the required default-off exclusivity warning.
- Actual binary fixture replay: exit 0, three JSON `ContactFrame` lines and the expected summary.
- Actual binary negative checks: `devices --grab`, `inspect DEVICE --grab`, `replay INPUT --grab`, and duplicate `record ... --grab --grab` all return usage exit 1 before command execution.
- Credential scan outside `target` and `.git`: 0 matches.
- No real input device was opened or grabbed; real-hardware behavior remains unverified.

### Finding disposition

| Finding | Re-review result |
| --- | --- |
| R1 signal-handler ownership and install state | **OPEN — blocking memory-safety race remains** |
| R2 recorder/header before optional grab | **CLOSED** |
| R3 ordered finalization and composite failures | **OPEN — fatal path still finishes after release and loses a cleanup result** |
| R4 ordered fallback destruction | **CLOSED** |
| R5 `--grab` command scope | **CLOSED** |

### R1 remains open — guard teardown does not synchronize an in-flight handler

The repair correctly makes the guard retain an `Arc` clone and rejects a
second active install. Those changes fix caller-owned lifetime and overlapping
installation, but they do not make reclamation of the raw handler pointer safe.

`termination_handler` loads `FLAG_PTR` and then dereferences it in
`crates/touchpad-linux/src/sys/ffi.rs:438-446`. Guard destruction restores the
two dispositions, clears `FLAG_PTR`, and then permits the last guarded `Arc` to
be released (`ffi.rs:490-510`). Restoring a disposition prevents a *new*
invocation; it does not wait for a handler that already loaded the non-null
pointer on another thread. The following safe execution therefore remains
possible:

1. a signal handler loads the old non-null pointer and is descheduled;
2. another thread drops the guard, restores dispositions, clears the global
   pointer, and releases the last `Arc`;
3. the in-flight handler resumes and dereferences freed storage.

The implementation itself documents arbitrary-thread signal delivery
(`ffi.rs:405-415`, `signals.rs:26-37`), and the public guard is not constrained
by the type system to a process that cannot race teardown. The statements that
the handler "can no longer run" immediately after `sigaction` restoration are
therefore incorrect. The same reclamation window also exists on the partial
installation rollback path if the first handler began executing before it was
restored.

Required before approval:

- Eliminate reclamation of a caller allocation from the async handler path
  (a process-lifetime static flag is the simplest shape), or provide a teardown
  protocol that demonstrably waits out every in-flight handler before freeing
  its target. Atomic pointer ordering alone is not lifetime synchronization.
- Make the public safe API enforce its concurrency assumptions rather than
  relying on prose that safe callers can violate.
- Correct the concurrency/safety claims and add an appropriate regression or
  model test for the chosen design. Do not add a timing-dependent test that
  intentionally invokes undefined behavior.

### R3 remains open — fatal cleanup still calls `finish()` after release

Signal and grab-failure paths now call recorder `finish()` before
`shutdown()`, and finalization failures affect the exit code. The fatal
stream/decoder/recorder path is still different:

- `EvdevRuntime::fail_open` calls only `recorder.flush()`, then ungrabs and
  closes the device (`crates/touchpad-linux/src/runtime.rs:724-741`).
- `cmd::record::finalize` subsequently takes the recorder and calls
  `recorder.finish()` (`apps/touchpadctl/src/cmd/record.rs:187-203`).
- `TraceWriter::finish()` is a genuinely fallible operation that calls
  `flush()` before marking the writer finished
  (`crates/touchpad-trace/src/writer.rs:189-208`); it is not a state-only marker.

This directly violates the documented and previously required invariant
"finish/flush before device release, never after". It is observable when the
fail-open flush fails transiently and the later finish succeeds: the command
can treat recorder finalization as successful even though it became successful
only after the device was released.

There is also a result-composition gap. `final_failure` uses the fail-open
report's `ungrab` and `close` values but never examines its `recorder_flush`
value (`record.rs:282-330`). The printed `cleanup:` line is built from the
second, already-closed `shutdown` report, so a fatal path can print release as
`n/a (already closed)` instead of the actual fail-open results
(`record.rs:223-240`). This contradicts the claim that every cleanup result is
merged and no diagnostic is lost.

Required before approval:

- Use one fatal finalization sequence in which the recorder's complete
  fallible finalization (including any best-effort recorder destruction needed
  to flush buffered bytes) finishes before ungrab and close. Preserve M4's
  immediate fail-open behavior by making that sequence part of fail-open, not
  by postponing `finish()` until the command regains control.
- Do not call `finish()` again after release. Carry its result in the structured
  cleanup report and merge the actual fatal-path recorder/ungrab/close results
  into both the final decision and truthful status text.
- Add a shared-timeline fatal-path test proving `finish < ungrab < close`, plus
  a command-level fault-injection test where fatal primary failure is combined
  with recorder finalization failure. Assert both diagnostics and the exit
  precedence.

### Closed findings

- **R2 closed:** command order is open/validate without grab, create recorder,
  flush header, attach recorder, optional checked grab, then read. Shared
  timeline and zero-grab failure tests pass.
- **R4 closed:** `EvdevRuntime::Drop` performs best-effort recorder flush before
  `DeviceHandle` field destruction; runtime and status-writer failure timeline
  tests pass.
- **R5 closed:** parser and actual binary reject `--grab` outside `record` and
  reject duplicates with usage exit 1.

### Re-review gate

M5 remains **not approved**. Repair only the two open M5 findings above, update
the false R1/R3 claims in `DESIGN_V2.md`, `README.md`, module documentation, and
`docs/M5_ACCEPTANCE.md`, rerun all gates, and stop for another independent
review. Do not start post-M5 gesture, output-backend, daemon, or desktop-policy
work.

## Final acceptance — 2026-08-16

### Decision

**M5 is approved.** The final repair closes R1 and R3 without reopening R2,
R4, or R5. This approval covers the Phase 1 CLI vertical slice only; it does
not authorize or claim completion of post-M5 gesture recognition, virtual
output, daemon/service integration, or desktop policy.

### R1 closed — no reclaimable handler target

The signal handler no longer loads or dereferences a pointer to caller-owned
storage. Its only data access is a store to process-lifetime static
`TERMINATION_REQUESTED`, so an already-running handler can safely resume after
guard teardown: no target is reclaimed. The public installer takes no caller
flag; the single-active-install invariant and structured second-install error
remain enforced. Runtime and CLI observe the static together with the
injectable test stop flag. The real `raise(SIGINT)` test and deterministic
post-teardown handler model tests pass.

Non-blocking precision note: an old invocation that resumes after teardown can
conservatively set the process static again. This is memory-safe and cannot
affect the one-install-per-process CLI flow. Documentation should avoid reading
"fresh install starts clean" as a cross-thread synchronization guarantee: a
`sigaction` restoration still does not join an already-running handler.

### R3 closed — one complete finalizer before release

`EvdevRuntime::shutdown`, fatal `fail_open`, and fallback `Drop` now all use the
runtime-owned recorder finalizer. It takes the recorder, calls fallible
`finish`, captures the event count, and destroys the recorder before ungrab and
close; consequently the recorder's best-effort Drop flush also happens before
device release. The command no longer calls `finish` after release.

Fatal and non-fatal paths expose one actual `ShutdownReport` carrying recorder
finish, event count, ungrab, and close results. That same report drives both the
printed cleanup line and exit precedence. Shared-timeline and combined-failure
tests prove `finish < recorder drop < ungrab < close`, preserve the primary
failure, and select recorder exit 7 ahead of device-release exit 6 and the
primary stop reason.

### Independent quality gates

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace`: pass, **368 tests total** (366 normal plus 2 doc
  tests), 0 failed.
- Actual CLI help and fixture replay: pass.
- Actual CLI grab-negative checks: pass; non-record use and duplicate
  `--grab` return usage exit 1.
- Credential scan outside `.git` and `target`: 0 matches.
- Tested compiler: rustc/cargo 1.97.1. The declared Rust 1.85 MSRV remains
  unverified because `rustup`/a 1.85 toolchain is not installed.

### Current-machine landing smoke (real hardware, no grab)

Environment: Manjaro Linux kernel 6.12.103, x86_64, KDE Wayland. The host user
is in the `input` group and has read/write access to `/dev/input/event12`.

- `touchpadctl devices` enumerated 15 event nodes, rejected non-touchpad nodes
  with reasons, and selected exactly one candidate:
  `CIRQ1080:00 0488:1054 Touchpad` at `/dev/input/event12`.
- `touchpadctl inspect /dev/input/event12` matched the independent kernel/udev
  and libinput identity: internal I2C Type-B buttonpad, five slots, BTN_LEFT,
  axes X `[0,3141]` and Y `[0,1842]`, resolution 24 units/mm. The calculated
  size (about 130.9 x 76.8 mm) matches libinput's 131 x 77 mm.
- Existing KDE per-device configuration contains `TapDragLock=true`; this is a
  useful system-experience baseline for later design work, not an M5 runtime
  dependency. `libinput list-devices` reports context defaults and must not be
  confused with compositor-applied KDE policy.
- Real non-exclusive SIGINT recording (no `--grab`) captured **4,908 raw
  events**, decoded **674 frames / 1,020 contacts**, and reported 0
  discontinuities and 0 diagnostics. Controlled stop returned exit 8 with
  recorder/ungrab/close all successful.
- Offline replay of that real trace forwarded exactly **4,908 events** and
  reproduced exactly **674 frames**.
- A separate real non-exclusive SIGTERM recording captured **1,510 raw
  events**, decoded **186 frames / 347 contacts**, and also returned exit 8
  with 0 discontinuities, 0 diagnostics, and fully successful cleanup.
- Evidence traces are temporary files under `/tmp`:
  `touchpad-m5-live-events-20260816.jsonl` and
  `touchpad-m5-live-sigterm-20260816.jsonl`.

### Still-unverified hardware behavior

The landing smoke deliberately did **not** use `--grab`; therefore real
exclusive ownership and release are not approved. Real device unplug handling
and a naturally occurring/injected kernel `SYN_DROPPED` resynchronization also
remain unverified. These omissions do not block M5 because its milestone gate
requires honest separation of automated, environment, and hardware evidence;
they must remain explicit before any broader real-hardware support claim.
