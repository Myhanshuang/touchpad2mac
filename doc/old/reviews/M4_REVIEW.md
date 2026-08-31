# M4 Review — Linux Device Boundary and Fail-Open Grab

Status: **APPROVED**

Reviewed: 2026-08-16 (Asia/Shanghai)

Scope: M4 only. M1–M3 remain approved. M5/CLI/signal handlers/output backends remain out of scope.

## Initial review verification (superseded by re-review 2)

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace`: pass, 260 tests total (258 normal tests plus 2 doc tests), 0 failed.
- No API key or other credential was found in the reviewed source/document set.
- The tests do not open or grab a real input device.

Passing mocks are not sufficient for approval because two mock contracts currently disagree with the Linux evdev UAPI/kernel implementation.

## Blocking findings

### R1 — The live fd never selects `CLOCK_MONOTONIC`

`event.rs` and `runtime.rs` treat every live timestamp as monotonic, but the syscall seam has no `EVIOCSCLOCKID` operation and `EvdevRuntime::open` never requests `CLOCK_MONOTONIC` on the fd.

The Linux evdev client is zero-initialized and `INPUT_CLK_REAL == 0`; therefore the default client clock is realtime. The kernel only changes it after `EVIOCSCLOCKID`. Current live events would be mislabeled as `Monotonic`, violating the M1/M2 time contract and making wall-clock adjustments look like fatal timestamp regressions.

Required:

- Add a mockable `EVIOCSCLOCKID` syscall seam and correct request encoding.
- Set `CLOCK_MONOTONIC` on the exact fd used by the runtime, before grab and before reading any event.
- Treat failure as an actionable open/setup error, close the fd, and never grab/start the runtime.
- Add mock tests for success, failure cleanup, call order, and absence of reads/grab after failure.
- Correct all documentation that claims evdev is monotonic “by construction.” It is monotonic only after the explicit ioctl succeeds.

Kernel evidence: `drivers/input/evdev.c` maps `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, and `CLOCK_BOOTTIME` only in `evdev_set_clk_type`; `include/linux/input.h` defines `INPUT_CLK_REAL = 0`.

### R2 — `EVIOCGMTSLOTS` return semantics are modeled incorrectly

`LinuxSys::ioctl_mt_slots` returns the raw ioctl return value. `EvdevSnapshotSource::read_mt_axis` requires that value to equal `slot_count * 4`, and `MockSys` is built to return that byte count.

The real kernel's `evdev_handle_mt_request` copies slot values and returns **0 on success**, not the number of copied bytes. Consequently every real snapshot with a nonzero slot count currently fails `SlotMismatch`; real `SYN_DROPPED` recovery cannot succeed.

The buffer protocol is also misstated: the required buffer is one leading code plus `slot_count` values (`slot_count + 1` `i32`s), not `slot_count + 2`, and the return value cannot be used to discover or validate the kernel slot count.

Required:

- Make the seam model success/failure, not a byte count, for `EVIOCGMTSLOTS`.
- Use the ABI-correct `(slot_count + 1) * sizeof(i32)` buffer.
- Derive/validate slot count from `ABS_MT_SLOT` on the same open fd and bound it with `MAX_SLOT_COUNT`; do not invent a return-byte validation the kernel does not provide.
- Make the mock match the real kernel (success is zero at the FFI boundary) and add a regression test that would fail under the old byte-count assumption.
- Correct §14.6–14.8 of `DESIGN_V2.md`.

Kernel evidence: `drivers/input/evdev.c::evdev_handle_mt_request` computes `max_slots = (size - sizeof(__u32)) / sizeof(__s32)`, writes `ip[1 + i]`, and returns 0.

### R3 — `struct input_event` is hard-coded to the 64-bit layout

`INPUT_EVENT_SIZE` is fixed at 24 and `KernelEvent::from_bytes` always reads two 8-byte time fields. The module explicitly claims this is the layout on both 32-bit and 64-bit Linux, which is false. Linux UAPI defines the layout conditionally by word size/time64 ABI; ordinary 32-bit layouts can be 16 bytes.

Required:

- Either implement target-correct decoding using the UAPI/libc layout for every supported Linux target, or explicitly restrict compilation/support to the actual supported architectures and fail at compile time elsewhere. General “Linux” support with a false 24-byte assumption is not acceptable.
- Keep unsafe confined to the FFI module if native layout conversion requires it; do not spread unchecked casts into the decoder/runtime.
- Add Linux ABI assertions against `size_of::<libc::input_event>()` and target-appropriate encode/decode tests. Retain portable mock tests.
- Correct the 32/64-bit documentation.

Kernel evidence: `include/uapi/linux/input.h::struct input_event` conditionally uses `struct timeval` or `__kernel_ulong_t` fields based on `__BITS_PER_LONG` and time64.

## Major findings

### R4 — Startup does not validate and run the same fd, and grabs too early

`EvdevRuntime::open` calls `device::probe`, which opens, validates, and closes one fd; it then opens the path again and uses the second fd without revalidation. Device removal/path reuse between those opens can attach the previously derived descriptor and slot count to a different device. It also issues `EVIOCGRAB(1)` before decoder/snapshot preparation, contrary to the documented startup order.

Required:

- Open once for a runtime session and perform capability/axis/slot validation on that exact fd.
- Prepare the clock, descriptor, decoder, and snapshot source first; issue the optional grab last, immediately before entering `Running`.
- Preserve enumeration's temporary probe behavior, but share a `probe_open_fd`/equivalent implementation so the rules cannot drift.
- Add mock assertions for one session open, same-fd queries, order, and cleanup on each preparation failure.

### R5 — A failed ungrab can issue `EVIOCGRAB(0)` twice

`DeviceHandle::ungrab` leaves `grabbed = true` when the ioctl fails. `shutdown`/`fail_open` first call `ungrab()` and then `close()`, whose implementation calls `ungrab()` again. This contradicts the documented “at most once” and means an error path is not idempotent at the syscall boundary.

Required:

- Track “release attempted” separately from “known grabbed,” or centralize shutdown so `EVIOCGRAB(0)` is attempted at most once even when it fails.
- Always close afterward so kernel fd teardown remains the fail-open guarantee.
- Preserve the first ungrab error in the shutdown report while still reporting close status.
- Add mock tests for failed ungrab during explicit shutdown, fatal decoder/resync cleanup, repeated shutdown, and Drop; assert one release attempt and one close.

### R6 — Buffered post-resync events can replay state older than the snapshot

If one `read` batch contains `SYN_DROPPED`, its recovery `SYN_REPORT`, and later events, the snapshot ioctl observes kernel state that already includes those later events. The runtime then continues feeding those older buffered events after atomically installing the newer snapshot. Repeated absolute values may appear harmless, but multiple tracking-id lifecycles can roll the decoder backward and emit false transitions.

Required:

- Define and implement a live recovery/drain rule that never applies events known to predate the installed snapshot. At minimum, events remaining in the current read batch after successful snapshot recovery must not be replayed.
- Add a regression with multiple post-boundary tracking-id lifecycles in one read; prove no stale lifecycle/frame is emitted after the discontinuity snapshot.
- Document the real evdev queue/snapshot ordering limitation and the chosen fail-closed synchronization boundary.

### R7 — Required ioctl response completeness is not consistently validated

The snapshot ignores `EVIOCGKEY`'s returned length, so a short response silently becomes “buttons not pressed” because the buffer was zero-filled. Probe bitset/name calls similarly accept arbitrary short lengths without distinguishing a complete response from truncated/mock-corrupt data. For resync, silently clearing a held physical button is unsafe.

Required:

- Validate that returned data covers every bit/field actually consumed (especially `BTN_LEFT` through `BTN_MIDDLE` during resync), or encode a stronger typed seam whose successful response is already complete.
- Add short-response mocks and require fail-closed snapshot behavior with no frame.
- Keep forward-compatible oversized buffers/results safe and bounded.

## Local “good touch experience” reference (read-only observation)

The current KDE/libinput configuration was inspected without opening or grabbing `/dev/input`:

- Device: `CIRQ1080:00 0488:1054 Touchpad` (KDE config group also records decimal vendor/product `1160/4180`).
- `pointerAcceleration=0.8`
- `naturalScroll=true`
- `scrollTwoFinger=true`
- `scrollEdge=false`
- `TapDragLock=true`

The user reports that the current touch experience is good. Preserve these as a calibration/A-B baseline for later pointer/scroll/tap milestones. They must **not** become live dependencies or be copied numerically as though KDE/libinput units equal this project's units. The master design still requires this runtime to own policy after grab; use the baseline to design measurable behavior profiles and comparison traces, not to let desktop settings control runtime behavior. M4 should only document this reference and must not implement policy engines or read desktop configuration at runtime.

## Approval gate

M4 remains unapproved until R1–R7 are fixed, documentation is corrected, strict checks pass, and the independent review is repeated. Do not start M5.

## Re-review 1 — 2026-08-16

Status: **CHANGES REQUESTED**

The first repair pass materially improved the implementation:

- R1 passes: the same session fd receives `EVIOCSCLOCKID(CLOCK_MONOTONIC)` before grab/read, and setup failure closes without grabbing.
- R4 passes: runtime opens once, probes that fd, prepares the decoder/snapshot, and grabs last.
- R5 passes: failed release is attempted once, close still runs, and explicit shutdown preserves the release error.
- R6 passes for the required boundary: the runtime detects the feed that installed a snapshot and discards the remainder of that already-read batch; lifecycle regression tests cover the behavior.
- Quality gates pass: fmt, strict all-target/all-feature clippy, and 285 workspace tests (including 2 doc tests), 0 failures.

R2, R3, and R7 still need a narrow correction:

### RR1 — `EVIOCGKEY` real return value is still discarded (R7 remains open)

`LinuxSys::ioctl_key_state` calls `ioctl_call` but discards its return value and unconditionally returns `bits_to_bytes(KEY_MAX)`. The accompanying trait/module/design documentation says the ioctl returns 0 on success. This is incorrect: `EVIOCGKEY` is routed through `evdev_handle_get_val`, which returns `bits_to_user(...)`, i.e. the number of bytes copied. Only `EVIOCGMTSLOTS` uses `evdev_handle_mt_request` and returns 0.

As a result, the new short-response fail-closed behavior exists only in `MockSys`; the real FFI can never report a short response to the snapshot layer.

Required: return the actual nonnegative ioctl result from `LinuxSys::ioctl_key_state`, keep the mock's byte-count contract, and correct all false `EVIOCGKEY returns 0` documentation. Add/retain a contract test that clearly distinguishes `EVIOCGKEY` byte-count semantics from `EVIOCGMTSLOTS` zero-on-success semantics.

### RR2 — The MTSLOTS undersized-buffer mock still contradicts the kernel (R2 partially open)

`MockSys::ioctl_mt_slots` and its test/docs claim `evdev_handle_mt_request` returns `-EINVAL` before writing when `num_slots` exceeds `max_slots`. The kernel loop actually writes `min(mt->num_slots, max_slots)` values and returns 0; it does not perform that size mismatch rejection.

The normal same-fd path is safe because `slot_count` comes from `ABS_MT_SLOT.max + 1`, but the mock must still reproduce the real truncation behavior instead of inventing an error.

Required: make the mock copy only the values fitting after the leading code and return `Ok(())`, remove/replace the false failure test, and correct `snapshot.rs`/`DESIGN_V2.md`. Preserve the same-fd slot-count invariant as the reason the production adapter receives a complete result.

### RR3 — The claimed general 32/64-bit event ABI support is not yet true (R3 partially open)

The repair selects layout only by pointer width. That misses UAPI semantic/layout variants: 32-bit time64 uses unsigned `__kernel_ulong_t` fields while the code decodes them as signed `i32`, and sparc64 has a 32-bit usec field plus padding rather than a second ordinary 64-bit `long`. A `size_of` assertion cannot catch wrong field interpretation when total size remains equal.

Required: either implement and test the ABI variants explicitly, or use a compile-time support restriction for live Linux FFI targets whose layout is actually implemented, as the original R3 explicitly allowed. Documentation must state the honest live target set and must not claim all 32/64-bit Linux. Portable replay/mock compilation may remain broader. No unverified cross-target claim is acceptable.

After RR1–RR3, rerun all gates and stop for another independent review. M5 remains forbidden.

## Re-review 2 — 2026-08-16

Status: **APPROVED**

The second, narrowly scoped repair closes the remaining kernel-contract and target-ABI findings:

- RR1 / R7 passes: `LinuxSys::ioctl_key_state` now preserves the real nonnegative ioctl return value, so production snapshot completeness checks see the actual copied byte count. The mock contract and regression test explicitly distinguish this from `EVIOCGMTSLOTS`' unit/zero-on-success contract.
- RR2 / R2 passes: the mock now copies `min(num_slots, max_slots)` values after the leading axis code and succeeds, matching the kernel's truncation behavior instead of inventing `EINVAL`. Production completeness continues to rely on the slot count derived from `ABS_MT_SLOT` on the same fd.
- RR3 / R3 passes: live Linux event decoding is honestly restricted at compile time to the implemented and verified x86_64 ABI. The 24-byte layout, two 8-byte time fields, libc size assertion, docs, and tests are consistent; unsupported Linux architectures are no longer claimed as supported.
- R1, R4, R5, and R6 remain closed from re-review 1. No regression was found in clock selection, same-fd startup validation, grab ordering/cleanup, or post-resync batch draining.

Independent final gates:

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass.
- `cargo test --workspace`: pass, **286 tests total** (284 normal tests plus 2 doc tests), 0 failed.
- Credential scan: 0 API-key matches outside build artifacts.
- Scope scan: no `apps/` directory or `touchpadctl`; M5 has not begun.
- Hardware boundary: tests remain mock/offline-only and do not open or grab a real input device.

All M4 review findings R1–R7 and follow-ups RR1–RR3 are closed. M4 is approved. This approval does not claim real-hardware validation and does not authorize or begin M5.
