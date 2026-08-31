# M5 Acceptance — CLI Vertical Slice and Phase 1 Handoff

Status: **implemented, pending external review**. This document is the
acceptance matrix for the M5 milestone (`MILESTONES.md` M5). It strictly
separates (1) automated tests, (2) environment observations, and (3)
real-machine verification — nothing here claims real-hardware validation.

## 1. Automated tests (no `/dev/input`, no root, no desktop session)

Nearly every automated test runs through the mockable `Sys` seam
(`touchpad-linux::sys::mock::MockSys`) and the hand-written trace fixtures.
**No test successfully opens or grabs a real device.** The exceptions are the
Linux FFI tests in `touchpad-linux::sys::ffi`, which deliberately exercise
the *real, side-effect-free* OS surfaces behind the seam so the seam itself
is verified against the actual kernel ABI:

- a real `sigaction` install/restore round-trip and a real `raise(SIGINT)`
  delivery (`real_sigint_records_the_stop_request`,
  `guard_drop_restores_the_previous_dispositions_and_resets_stop_state`);
- a real `read_dir` on a nonexistent path (`/definitely/not/a/real/path/…` →
  `ENOENT`, no device touched);
- a real `open(2)` attempt on a nonexistent device node
  (`/dev/input/event999999` → `ENOENT`/`EACCES`; nothing is opened).

These are side-effect-free: they never open or grab a device and never
modify process state outside the signal tests' own (serialized) handler
install/restore.

Gates (all pass at the end of M5, after the R1–R5 fixes):

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace                       # 368 tests, 0 failures
```

Manual acceptance commands (also run at the end of M5):

```text
cargo run -p touchpadctl -- --help                                  # exit 0, --grab warning
cargo run -p touchpadctl -- replay crates/touchpad-trace/tests/fixtures/single_contact.jsonl
# exit 0; three JSON ContactFrame lines on stdout; summary on stderr
```

M5 test matrix highlights (full list in `DESIGN_V2.md §16.7`):

| Requirement | Proof |
| --- | --- |
| CLI help | `args::tests::help_warns_explicitly_about_grab_risks`, `tests/cli.rs::help_command_exits_zero_and_warns_about_grab` |
| No-device behavior (exit codes, no panic) | `devices` no nodes → 4; missing `/dev/input` → 2; permission → 3; `inspect`/`record` missing node → 2, permission → 3, non-candidate → 4 |
| Fixture replay smoke | `tests/cli.rs::fixture_replay_smoke_through_the_command_runner` (single_contact → 3 frames), `cmd::replay` clean fixtures |
| Corrupted trace | `replay` corrupted line → 5; schema-too-new → 5; time regression → 5; missing file → 5; header-only → 0 |
| Record pipeline ordering (raw write before decode) | decoder-failure tests keep **all** read events in the trace (7 events incl. the failing feed); drained post-resync events also recorded |
| R1: process-lifetime static signal state (no caller-owned storage on the async handler path) | handler's only side effect is a store to a never-reclaimed `'static` `AtomicBool`; **deterministic model tests** `in_flight_handler_resuming_after_teardown_touches_only_static_memory` / `..._is_safe_by_construction` prove the previously-unsafe interleaving (fire → guard teardown → fire again) touches only static memory; `second_install_is_rejected_with_structured_error`; `fresh_install_succeeds_after_the_first_guard_is_dropped` (FFI + `signals` wrapper levels); real `raise(SIGINT)` sets the static, guard drop restores dispositions and resets it |
| R2: recorder/header ready and flush precede the grab | `cmd::record::header_flush_precedes_grab_in_the_shared_timeline` (shared timeline: header flush < `EVIOCGRAB(1)` < first read) |
| R2: unwritable/header-flush-failing output → zero grabs | `recorder_output_failure_is_actionable_with_zero_grabs`, `header_flush_failure_issues_zero_grabs`; runtime `grab_is_checked_idempotent_and_rejected_after_step_or_shutdown` |
| R3: truthful composite failures, exit 8 only on full success | `recorder_finish_failure_returns_recorder_exit_with_cleanup` (finish failure → 7); `failed_ungrab_with_successful_close_is_reported` (→ 6, both diagnostics); `close_failure_is_reported` (→ 6); `primary_stream_error_combined_with_cleanup_failure_preserves_both` (decoder error + ungrab failure both in message); **`fatal_primary_failure_combined_with_recorder_finalization_failure` (fatal decoder error + finish failure → 7, both diagnostics and exit precedence)**; runtime `shutdown_with_finish_failure_reports_it_and_still_releases_in_order` |
| R3: recorder finish (+ best-effort destruction) before ungrab/close on **every** path | runtime performs the whole sequence in one place: `signal_stop_orders_finish_before_ungrab_before_close` (finish < drop < ungrab < close, shared timeline); **`fatal_path_orders_finish_before_ungrab_before_close` (shared-timeline fatal path: finish < drop < ungrab < close, results in the fail-open report)**; the command never calls the fallible `finish` after the release; fatal-path cleanup line prints the actual fail-open ungrab/close results (no "n/a (already closed)" misreport) |
| R4: ordered fallback Drop (recorder finalization before device release) | runtime `drop_finalizes_recorder_before_releasing_the_device` (finish < drop < ungrab < close); `cmd::record::status_writer_failure_after_recorder_attachment_uses_ordered_fallback` (failing status writer + shared timeline) |
| R5: `--grab` record-only, duplicates rejected | `args::tests::grab_is_rejected_for_every_command_except_record`, `duplicate_grab_is_rejected`; `tests/cli.rs::grab_is_a_usage_error_for_non_record_commands`; actual-binary checks in the review session |
| Signal stop | FFI `real_sigint_records_the_stop_request` (real `raise(SIGINT)` sets the process-lifetime static); EINTR+stop requested → graceful (`RuntimeError::Interrupted`, device left open); exit 8; stop state polled between steps |
| Repeated shutdown / finish-ungrab-close order | `runtime` shared-timeline test (`TimelineSys` + `MarkerRecorder`): finish < drop < ungrab < close; repeated shutdown adds no syscalls and does not repeat the recorder finalization; ungrab attempted at most once even on failure (M4 R5 kept) |
| Same decoder for replay | `recorded_trace_replays_through_the_same_decoder`; replay drives `TypeBDecoder` via `ReplayDriver` (no second state machine) |
| `--grab` defaults off, explicit opt-in | help text warning; `record` without `--grab` issues no `EVIOCGRAB(1)`; with `--grab` exactly one grab and one release |

Fixture corpus:

- Shared (`crates/touchpad-trace/tests/fixtures/`): `single_contact`,
  `multi_slot`, `buttons`, `missing_resolution`, `dropped_recovery` — the M3
  raw-event corpus, replayed by the CLI tests.
- CLI (`apps/touchpadctl/tests/fixtures/`): `empty.jsonl` — header-only trace
  (zero events / zero frames, exit 0), an edge case not covered by the shared
  corpus.

## 2. Environment observations (this session, side-effect-free only)

- Toolchain: rustc/cargo 1.97.1, x86_64. The workspace declares
  `rust-version = 1.87` (raised from 1.85 during the M6 re-review, R6: the
  locked graph — zbus 5.19 and the zvariant family — declares 1.87, so 1.85
  was never the real minimum). The declared MSRV has **NOT been independently
  tested yet**: the quality gates in this milestone ran on 1.97.1 only. A
  real 1.87 toolchain gate must be run before claiming 1.87 is verified.
- `/dev/input` does **not** exist in this session (checked with a directory
  listing only; no device node was opened). Consequently live commands
  cannot be exercised here at all — the no-device behavior is proven by the
  automated mock tests above.
- These observations are environment facts, not validation.

## 3. Real-machine verification (NOT performed; out of scope for M5)

The following must be verified by an external reviewer on real hardware,
separately from the automated tests, before any claim of real-hardware
support:

- correct built-in touchpad identification via `devices`/`inspect`;
- capabilities and axis resolution match `evtest` / kernel information;
- real `--grab` exclusivity (system stops receiving touchpad events) and
  release on `SIGINT`/`SIGTERM`/EOF;
- real `SYN_DROPPED` ioctl resynchronization while recording;
- real unplug (EOF) behavior and permission diagnostics on a real system;
- signal-driven cleanup against a physical touchpad (Ctrl-C during record).

## 4. Known honest limitations

- Live Linux input is implemented and verified only for **x86_64** (M4 RR3);
  other Linux ABIs fail at compile time.
- `SIGKILL`, a kernel crash, or a hard power loss cannot run userspace
  cleanup; the kernel releases the grab when the fd closes at process exit,
  but the ordered recorder-finalization→ungrab→close sequence is only
  guaranteed on paths this process can run.
- Offline `replay` cannot resynchronize a `SYN_DROPPED` trace (no kernel
  snapshot source offline); it fails with a structured error after printing
  the frames decoded before the drop.
- No output backend (Wayland/libei, X11, uinput), pointer/scroll/tap/drag/
  gesture algorithms, or desktop-configuration reads exist in this milestone.
