# M10 Review — Bounded, Fail-Open Live Takeover Slice

Date: 2026-08-17  
Decision: **REJECTED — repair M10; do not run live takeover**

The overall architecture is directionally correct: a streaming M6 adapter feeds the approved M7–M9 arbiter through a first-fault bridge; `step_deferred` keeps resources available for the coordinator; recorder/header readiness precedes grab; the loop has a bounded poll quantum; and explicit shutdown removes the bridge before recorder finalization, ungrab, and close. Independent debug and release gates both pass with 781 tests. The passing suite misses several live-only branches at precisely the safety boundary M10 exists to establish.

## Blocking findings

### R1 — Critical: SIGINT/SIGTERM interrupting `poll(2)` is misclassified as a stream failure

`LinuxSys::poll` correctly surfaces `EINTR` as `SysError::Interrupted`, but `run_loop` maps every readiness error directly to `StopReason::Stream(RuntimeError::Read(error))`. With the installed non-`SA_RESTART` handler, a normal Ctrl-C/SIGTERM while the loop is idle is expected to interrupt `poll`; the command then exits as stream failure (6), not the documented clean controlled stop. The existing signal test only sets the flag while returning `Ok(false)`, so it never exercises the real branch.

After a readiness `Interrupted`, re-check both stop sources: requested stop → `StopReason::Signal`; unrequested EINTR → a structured poll/stream failure. Add exact tests for both. Preserve the ordered cleanup and cleanup-failure precedence.

### R2 — Critical: device HUP/ERR readiness is ignored until the deadline

The real `poll` implementation returns ready only for `POLLIN`. An unplugged/failed evdev fd may wake with `POLLHUP` and/or `POLLERR` without `POLLIN`; current code converts that to `Ok(false)` and repeats until the maximum duration instead of immediately reading/surfacing EOF or failure and failing open. `POLLNVAL` is likewise silently treated as idle.

Classify revents explicitly. `POLLIN`, `POLLHUP`, and `POLLERR` must lead to immediate nonblocking progress that surfaces the actual read/EOF error; `POLLNVAL` must be an immediate structured error; timeout alone is idle. Add deterministic unit coverage for each flag and combinations, plus a takeover regression proving unplug/hangup initiates cleanup without waiting for the deadline.

### R3 — High: real server-interruption diagnostics are cleared before the coordinator reads them

`finalize` calls `bridge.release_all()` and only afterward extracts the streaming session and calls `take_server_interruption`. The real `PortalOutputSink::release_all_detailed` clears `self.interruption`, so a real DevicePaused/DeviceRemoved/SeatRemoved/Disconnect primary is lost and flattened to generic semantic-output failure (exit 6). The fake session does not clear its interruption on release, so `server_interruption_is_a_structured_output_fault` gives a false pass.

Capture the structured interruption before release mutates the session, then run cleanup. Align the fake lifecycle with the real adapter or add a test through `PortalStreamingOutput<FakePortal, FakeTransport>` that proves the category survives the actual release behavior. Also consume/preserve `take_cleanup_error` rather than exposing an otherwise dead structured accessor; avoid duplicating a diagnostic already carried by `ArbiterSinkError`, but do not flatten away its category.

### R4 — Medium: three mandatory takeover flags silently accept duplicates

`parse_takeover` counts only `--takeover` and `--confirm`. Repeated `--output-qualified` is silently accepted, while repeated `--profile` and `--max-duration-seconds` overwrite their prior values. This contradicts the CLI contract and documentation that every repeated takeover flag is rejected. The duplicate test covers only the two correctly counted flags.

Reject repeats of all five mandatory flags, independent of whether repeated values agree. Add exact parser/public CLI regressions for `--output-qualified`, `--profile`, and `--max-duration-seconds` duplicates, including conflicting values.

### R5 — High: the claimed multiple-explicit-release failure test does not fail an explicit release

`multiple_cleanup_failures_preserve_all_diagnostics_and_precedence` scripts only `[Ok(())]`: the initial Left down consumes that result, then the cleanup Left up sees an exhausted script and succeeds. Only the wrapped `release_all` fails. The test name/comment and M10 handoff therefore claim coverage that is not present; it also exercises only one owed explicit release, while M10_TASK §9 requires multiple explicit-release failures plus wrapped cleanup and later recorder/device failures.

Drive a legitimate Left+Right held state through the decoder/arbiter, accept both downs, reject both cleanup ups, fail wrapped output cleanup, recorder finish, ungrab, and close. Assert the returned diagnostic identifies both explicit failed events separately and preserves every later failure with documented precedence. Retain a separate success/retry/idempotence case.

### R6 — Medium: the real streaming factory touches D-Bus/libei before device open

`run` creates the real streaming output before `EvdevRuntime::open`, and `RealStreamingOutputFactory::create` immediately connects to the session bus and loads libei. This violates M10_TASK §4's external preparation order (device open/validation first, then output probe/prepare) and can return a portal/libei failure even when the explicitly named device is missing or invalid. The device-open test checks only `prepare_calls`, not factory-side external work.

Make real session construction side-effect-free/lazy so session-bus connection and libei loading occur inside `prepare` after the device has opened successfully, or refactor the runtime to install its sink after open. Add an observable factory/preparation timeline test and document object allocation separately from external preparation. A device-open failure must perform zero D-Bus/libei/output access and retain device-error precedence.

## Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 781 tests, 0 failed.
- `cargo test --release --workspace --locked`: pass, 781 tests, 0 failed.
- Credential-pattern scan outside generated/cache directories: 0 files.
- No live `/dev/input`, grab, portal/libei session, output emission, or system-setting operation was performed.

## Repair scope

Repair R1–R6 and add the exact regressions above. Preserve the approved M1–M9 semantics, M10 profile, foreground/bounded CLI, output-before-recorder-before-ungrab shutdown order, deferred cleanup, and no-late-output bridge. Correct `DESIGN_V2.md`, `MILESTONES.md`, README, acceptance text, test counts, and any premature code-complete/review-pass claim. Keep all work fake/offline; do not run live commands and do not start M11.

---

## Re-review 1 — 2026-08-17

Decision: **APPROVED FOR USER LIVE ACCEPTANCE — M10 code complete; stop before M11**

R1–R6 are closed:

- readiness `EINTR` re-checks the installed stop sources and distinguishes a controlled signal from an unrequested interruption;
- real poll revents explicitly classify input/hangup/error as immediate progress, invalid fd as an immediate error, and only a pure timeout as idle;
- the coordinator captures the real structured server interruption before release clears it, while cleanup diagnostics remain preserved without duplicate reporting;
- all five mandatory takeover flags reject repeats, including equal and conflicting value forms;
- the integrated multi-failure test now drives legitimate simultaneous Left+Right ownership, rejects both explicit ups, and preserves wrapped-output, recorder, ungrab, and close failures;
- real streaming factory construction is side-effect-free; session-bus connection and libei loading are deferred to `prepare` after device open/validation.

Independent inspection also confirms the original safe slice remains intact: no semantic event before grab, recorder header flush before grab, first output fault stops later frames from the same batch, idle sessions remain duration-bounded, and every explicit exit converges on output release → recorder finalization → ungrab → close.

### Final static verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 792 tests, 0 failed.
- `cargo test --release --workspace --locked`: pass, 792 tests, 0 failed.
- Credential-pattern scan outside generated/cache directories: 0 files.
- No live `/dev/input`, `EVIOCGRAB`, portal/libei session, output emission, or system-setting operation was performed.

### Acceptance boundary

M10 is approved as code and is ready for the user-run procedure in `docs/M10_ACCEPTANCE.md`. The output backend and M10 remain `experimental/live-unqualified` until the user records the M6 calibration evidence and completes the ordered 10-second, 60-second, and 300-second runs. This review does not approve an unbounded mode, daemon/autostart behavior, or M11 work.
