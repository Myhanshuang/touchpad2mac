# M6 Review — KDE Wayland Output Backend Qualification

Date: 2026-08-16  
Decision: **REJECTED — repair M6; do not start M7**

The new `touchpad-desktop` boundary, dry-run command, fake seams and documentation are a useful M6 skeleton. The independent static gates pass when using dsh's workspace-local Cargo cache, and the real non-emitting probe correctly observes the current KDE Wayland session. The real emission path is not safe or lifecycle-complete enough to run yet, so `output-probe --emit` was deliberately not executed.

## Blocking findings

### R1 — Critical: the supposedly safe FFI wrapper is unsound

`crates/touchpad-desktop/src/lib.rs:61` exports `ffi` publicly. `EiContext`, `EiSeat`, `EiDevice`, and `EiEvent` are all `Clone + Copy` (`ffi.rs:129–142`), while safe public methods accept those copied handles and perform ownership operations such as `event_unref`, `seat_unref`, `device_unref`, and `unref` (`ffi.rs:471–478`, `508–512`, `528–532`, `591–597`). Safe Rust can therefore copy a handle, unref it twice, or use the copied handle after unref. Privacy of the raw pointer does not enforce the documented ownership invariant.

This is a soundness defect, not merely a public-API preference. Repair the boundary so safe code cannot duplicate/release/use owned libei references illegally. Acceptable directions include non-`Copy` RAII owners plus explicit borrowed handles, or an entirely private unsafe API whose safe owner types encode lifetimes and one-time destruction. Keep unsafe localized and add compile/runtime-independent tests for ownership/lifecycle where possible.

### R2 — High: real Ctrl-C/SIGTERM does not use the advertised cleanup path

`apps/touchpadctl/src/main.rs:38–60` installs the termination handler only for `Command::Record`; every `OutputProbe`, including `OutputProbe { emit: true }`, keeps the default signal dispositions. The cancellation closure in `cmd/output_probe.rs` therefore cannot observe a real SIGINT/SIGTERM: the process is terminated by the kernel instead of returning exit 8 and running `release_all_detailed`.

This contradicts the CLI warning, `docs/M6_ACCEPTANCE.md`, and the M6 cleanup contract. Install controlled signal handling for the emitting form only (dry-run need not install it), prove the command classification, real signal flag observation, cleanup ordering, and restoration behavior. Authorization/handshake waits must also become cancellation-aware or document and test the bounded delay before cleanup.

### R3 — High: EIS server state is never pumped after the handshake

The only consumer of `Transport::wait_event` is `PortalOutputSink::handshake` (`sink.rs:222–276`). Once the sink reaches `Emulating`, submit/release directly call emission functions and never dispatch queued `DEVICE_PAUSED`, `DEVICE_REMOVED`, `SEAT_REMOVED`, or `DISCONNECT` events. Consequently the native transport's `resumed` set remains stale, live pause/removal/disconnect cannot become the promised structured failure, and the short test may continue writing without observing server state changes.

Add a nonblocking/pollable transport-pump step before/after logical emission frames and during interruptible waits. It must update device lifecycle, transition the sink out of `Emulating` on pause/removal/disconnect, reject subsequent output, and still execute cleanup. Tests must script these events after a successful handshake—not only during it—and prove no later wire event is emitted. Also clarify how libei outgoing data is flushed and how write-side/disconnect errors are surfaced; the current native emission wrappers all return `Ok(())` around `void` libei calls.

### R4 — High: prepare failures discard cleanup failures

On SelectDevices/Start, ConnectToEIS, transport-connect, or handshake failure, `PortalOutputSink::prepare` invokes `release_all_detailed` and discards its result (`sink.rs:185–215`). The original error is returned alone; the stored `cleanup_error` dies with the sink. This violates the explicit requirement to preserve the primary failure and all cleanup diagnostics.

Return one structured composite result or otherwise carry both errors to the caller. Add fault-injection tests for each preparation stage combined with transport disconnect and/or session close failure. Preserve error category/exit precedence without flattening away the primary cause.

### R5 — High: scroll stop violates per-axis lifecycle

`HeldState` records only a single `scroll_deltas_sent` boolean (`held.rs:22–29`, `112–115`), and every `ScrollEnd` sends `scroll_stop(device, true, true)` (`sink.rs:365–374`). The fixed probe emits only Y deltas, so it nevertheless stops X as well. libei tracks scrolling per axis; stop/cancel calls must reflect axes that actually received nonzero deltas.

Track X/Y activity separately for each scroll interaction, stop only active axes, handle zero deltas, and reset axis state on end/cancel/release. Add x-only, y-only, two-axis, zero-delta, repeated-lifecycle, partial-send, and forced-release tests.

### R6 — High: declared MSRV and resolved dependency disagree

The workspace still declares Rust 1.85 (`Cargo.toml:17`), while `Cargo.lock` resolves zbus 5.19.0 and that crate declares Rust 1.87. Documentation acknowledges the mismatch, but a `rust-version` declaration is a build contract, not just descriptive text. Either pin a maintained compatible zbus release and verify 1.85, or explicitly raise the workspace `rust-version` to the real minimum and update all acceptance/docs. Do not leave the manifest claiming 1.85 while the locked graph rejects it.

## Non-blocking but required cleanup

- The real device type is not queried. Relative coordinates are logical pixels only for a virtual libei device; add/query `ei_device_get_type` and reject or explicitly handle physical-device millimetres before claiming the unit mapping.
- Portal response timeout abandons a helper thread until process exit. Avoid an accumulating blocked-thread/session leak or explicitly bound/close the outstanding request; test timeout cleanup.
- dsh left a 104 MiB workspace-local `.cargo-home` cache that was not listed in its handoff. Remove generated review/build cache from the project and ensure it is ignored rather than delivered as source.
- `FakeTransport::disconnect` assigns `self.device = None` twice. Harmless, but remove the noise during repair.

## Independent verification

- `cargo fmt --check`: **PASS**.
- Plain `cargo clippy --workspace --all-targets --all-features -- -D warnings`: could not fetch the newly introduced registry graph in the restricted reviewer shell. This is an environment/network failure, not a source diagnostic.
- `CARGO_HOME=/home/acacia/touchpad/.cargo-home cargo clippy --offline --workspace --all-targets --all-features -- -D warnings`: **PASS**, 0 warnings.
- `CARGO_HOME=/home/acacia/touchpad/.cargo-home cargo test --offline --workspace`: **PASS**, 429 tests, 0 failed.
- Real `target/debug/touchpadctl output-probe` dry-run in the host KDE Wayland session: **PASS**; session bus reachable, RemoteDesktop v2, device types 7 with pointer, `libei.so.1` loadable, backend remains `experimental/unqualified`.
- Credential-pattern scan outside `target` and `.cargo-home`: **0 files**.
- Real `output-probe --emit`: **NOT RUN** because R1–R5 affect that live path.

## Repair acceptance

M6 may be re-reviewed only after all R1–R6 items are fixed, regression tests are added, generated cache is removed/ignored, and the three normal workspace gates pass from the documented dependency setup. The reviewer will then repeat dry-run and perform the bounded real `--emit` authorization/cancel/signal/A-B procedure. M7 remains blocked until M6 is explicitly approved.

---

## Re-review 1 — 2026-08-16

Decision: **REJECTED AGAIN — repair M6; do not run `--emit`; do not start M7**

The first repair substantially improves R1–R6, raises the manifest MSRV honestly to 1.87, adds signal coverage, separates scroll axes, preserves prepare cleanup failures, and adds post-handshake pump seams. Independent gates pass: fmt pass, clippy pass with 0 warnings, and 469 tests pass. Four live-path defects remain.

### R7 — Critical: RAII owners can still outlive the dynamically loaded library

`EiContext`/`EiSeat`/`EiDevice`/`EiEvent` store copied unref function pointers but carry no lifetime tied to `Libei` (`ffi.rs:158–205`). `Libei::new_sender(&self) -> EiContext` and the ref/get-event methods likewise return owners with no borrow lifetime (`ffi.rs:545–588` and subsequent ref methods). Crate-private visibility reduces exposure but does not make the safe abstraction sound: safe crate code can create an owner, drop `Libei`, then drop the owner and call an unref address in an unloaded shared object.

Encode the library lifetime in every owner/borrowed handle (`EiContext<'lib>`, etc.), or make one RAII root own the `Arc`/library guard so unloading is impossible while any owner exists. Documentation and “only current caller behaves” are not a type-level guarantee. Add a compile-fail ownership/lifetime test or an equivalent structural proof; keep the native transport ergonomics sound without self-referential borrowing.

### R8 — High: native pump can strand already-queued libei events

`NativeTransport::wait_event` polls the fd before calling `ei_get_event`. After one `ei_dispatch`, the function returns the first mapped event and can leave additional mapped events in libei's internal queue. On the next call the kernel fd may no longer be readable, so `poll(..., 0)` returns no events and `pump()` stops without reading the internal queue. The fake queue cannot reproduce this layering error.

Drain `ei_get_event` before polling, and after dispatch drain all queued events into a native pending queue or return mechanism. Prove that two or more events from one dispatch are all surfaced even when the following poll reports no readiness. Also reconcile the flush claim: current `pump()` does not call `ei_dispatch` when zero-time poll has no `POLLIN`, so the code and documentation disagree about outgoing flushing. Base the implementation on libei's actual API contract and add a native-adapter seam/test below raw FFI so the algorithm is testable without real emission.

### R9 — High: explicit zero-axis ScrollEnd still sends an invalid no-axis stop

The repair correctly tracks active axes, but `send(ScrollEnd)` always calls `transport.scroll_stop(device, stop_x, stop_y)` (`sink.rs:482–488`). With a zero-only interaction, this is `scroll_stop(false, false)` followed by a frame. `release_events` avoids the call only during forced cleanup; the normal explicit `ScrollEnd` path remains wrong.

When neither axis is active, `ScrollEnd` must be a local lifecycle marker with no wire stop and no frame, analogous to `ScrollBegin`. Add a direct begin → zero delta → end wire-log regression test, not merely a forced-release test.

### R10 — High: successful live emission discards the real outcome

`PortalDesktopOutput::emit_pattern` consumes `pattern_result` only to derive an optional error, then returns a newly defaulted `EmitOutcome` on success (`desktop.rs:105–121`). This loses `steps_emitted`, `wire_events`, and `skipped`; the CLI would report a successful real pattern as 0 steps / 0 events.

Preserve and return the exact successful `EmitOutcome`, changing only fields that truly need authoritative replacement. Add a test around the real `PortalDesktopOutput` orchestration logic through injected portal/transport factories so nonzero counts and skipped capabilities survive cleanup. Do not rely on `FakeDesktopOutput`, which bypasses this code entirely.

### Re-review 1 verification

- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass, 0 warnings (fresh locked dependencies fetched outside the restricted sandbox).
- `cargo test --workspace`: pass, 469 tests, 0 failed.
- Real `--emit`: not run because R7–R10 affect the live path.

R1–R6 remain subject to regression review; the next repair must not weaken them. M6 remains `experimental/unqualified` and M7 remains blocked.

---

## Re-review 2 — 2026-08-16

Decision: **REJECTED AGAIN — repair R11 only; do not run `--emit`; do not start M7**

R7, R9, and R10 are repaired correctly. R8 now drains the complete libei queue and flushes through an unconditional dispatch, but the pending-queue implementation advances lifecycle bookkeeping before the corresponding events are delivered.

### R11 — High: queued events mutate transport state before delivery

`NativeTransport::drain_internal_queue` maps every event immediately and pushes only the already-mapped `TransportEvent` into `pending`. `map_event` is not a pure conversion: it adds/removes seats and devices and changes the resumed set. Thus a single dispatch containing multiple lifecycle events applies the state of the *last* event before the caller has observed the first event.

This breaks the transport's event/state contract. For example, a single dispatched batch containing `SeatAdded`, `DeviceAdded`, `DeviceResumed`, then `DevicePaused` or `DeviceRemoved` leaves `resumed` empty (and possibly removes the device) before `DeviceResumed` is returned. When `PortalOutputSink::handshake` receives that earlier resume event and calls `start_emulating`, `require_device` fails even though the pause/removal has not yet been delivered. Similarly, an added seat can be removed internally before the caller receives `SeatAdded` and calls `bind_capabilities`.

Queue raw owned events (or a deferred transition carrying enough owned data) and perform mapping plus state mutation exactly when each event is popped for delivery. Preserve the R8 guarantee that every event from one dispatch is retained in order and that no event is stranded behind fd readiness. Add native-adapter regressions that prove, between successive `wait_event` calls:

- `SeatAdded` is bindable until `SeatRemoved` is actually delivered;
- `DeviceResumed` permits `start_emulating` until a queued `DevicePaused`/`DeviceRemoved` is actually delivered;
- after the pause/removal delivery, emission is rejected;
- a queued disconnect becomes terminal only when delivered, without losing prior queued events.

Do not weaken R7–R10 or broaden M6. Do not run real emission.

### Re-review 2 verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 481 tests, 0 failed.
- Real host-session `target/debug/touchpadctl output-probe` dry-run: pass; KDE Wayland session bus reachable, RemoteDesktop v2 with pointer available, `libei.so.1` loadable.
- Credential-pattern scan outside generated/cache directories: 0 files.
- Real `--emit`: not run because R11 affects the live handshake and lifecycle path.

---

## Re-review 3 + live qualification attempt — 2026-08-16

Decision: **CODE REVIEW PASSED, LIVE GATE FAILED — repair R12; M6 remains unqualified**

R11 is repaired correctly: the pending queue owns raw `EiEvent`s and maps each event only when it is popped for delivery. The new adapter tests exercise the required state between deliveries. Independent static gates all pass. The reviewer therefore advanced to the bounded real desktop-output probe; it failed before portal authorization or any emitted input.

### R12 — High: generated portal request token is not a valid D-Bus object-path element

The real command `target/debug/touchpadctl output-probe --emit` exited 9 after the countdown with `internal error: Invalid object path`. `ZbusPortal::next_token` generates `m6-<pid>-<counter>`, then `request_path` inserts that token directly into `/org/freedesktop/portal/desktop/request/<sender_safe>/<token>`. A D-Bus object-path element cannot contain `-`, so `MatchRule::builder().path(...)` rejects the locally constructed path before `CreateSession` or any output.

Generate portal tokens from the D-Bus object-path-safe alphabet (letters, digits, underscore), validate the complete predicted request path with zvariant before registering the match rule, and return a diagnostic that identifies path construction rather than the current context-free `Invalid object path`. Add unit tests proving every generated token and predicted path are valid, unique, and consistent with the portal naming convention. Also audit `CreateSession` options against the installed/official portal contract: it requires distinct request `handle_token` and session `session_handle_token`; add the session token if absent, with the same safe-character and uniqueness guarantees. Keep request/session tokens distinct and do not add `handle_token` to non-request methods unless the method contract permits it.

After static gates pass, do not claim qualification from tests alone. The reviewer must rerun the real bounded `--emit` path. Do not access or grab `/dev/input` and do not start M7.

### Re-review 3 verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 485 tests, 0 failed.
- Real `target/debug/touchpadctl output-probe --emit`: **FAIL**, exit 9, `Invalid object path`; failure occurred before authorization/emission.

---

## Re-review 4 + live qualification attempt — 2026-08-16

Decision: **STATIC REVIEW PASSED, LIVE GATE FAILED — repair R13; M6 remains unqualified**

R12's token/path repair is correct: request and session tokens are distinct and object-path-safe, the portal sender component matches the installed implementation, `ConnectToEIS` uses an empty options dictionary, and constructed paths are validated with contextual errors. Independent static gates pass. The real probe now advances beyond the former invalid request-path failure and receives the `CreateSession` response, but rejects its result before authorization or input emission.

### R13 — High: `CreateSession.session_handle` is decoded as `o`, but the portal ABI returns `s`

The real `target/debug/touchpadctl output-probe --emit` exits 2 with `RemoteDesktop portal unavailable: session_handle is not an object path: incorrect type`. The installed `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml` explicitly declares the `CreateSession` response dictionary's `session_handle` as string (`s`), noting that it contains an object path but was historically implemented with the wrong D-Bus type and remains `s` for compatibility. The code instead calls `OwnedObjectPath::try_from(OwnedValue)` directly, which correctly rejects the received string value as the wrong zvariant type.

Decode the response entry according to the wire ABI as a string first, then validate that string's contents as an `OwnedObjectPath` before storing `PortalSession`. Preserve distinct diagnostics for missing key, wrong D-Bus value type, and syntactically invalid path contents. Add pure response-decoding tests for: valid `s` value succeeds; missing key fails; non-string value fails with its actual value/signature context; string containing an invalid path fails; and the exact installed v2 compatibility case is documented. Avoid claiming that the response is wire type `o`.

Preserve R1–R12, run all gates, and leave the real `--emit` rerun to the reviewer. Do not access/grab `/dev/input` and do not start M7.

### Re-review 4 verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 491 tests, 0 failed.
- Installed portal XML: `CreateSession` result `session_handle` is documented as `s` containing an object path for backward compatibility.
- Real `target/debug/touchpadctl output-probe --emit`: **FAIL**, exit 2, response value has `incorrect type` for direct `OwnedObjectPath` conversion; no input emitted.

---

## Re-review 5 + successful live protocol qualification — 2026-08-16

Decision: **M6 APPROVED FOR DEVELOPMENT; LIVE OUTPUT PATH PASSED; TAKEOVER CALIBRATION REMAINS GATED**

R13 correctly decodes the historical `s` wire ABI and validates its contents as an object path. Independent review found no remaining R1–R13 blocker. The third bounded real probe completed the KDE RemoteDesktop → EIS → libei sender path successfully.

### Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 496 tests, 0 failed.
- Real `target/debug/touchpadctl output-probe --emit`: pass, exit 0.
- Negotiated: relative pointer, primary button, secondary button, pixel-precise smooth scroll.
- Completed: 6 bounded logical steps, 11 wire events, 0 capability skips.
- Portal authorization, session creation, EIS connection, libei handshake, emission, release, disconnect, and session close all completed without a reported error.
- No physical `/dev/input` device was opened, read, or grabbed by this probe.

### Qualification boundary

The real protocol/output lifecycle is accepted. The reviewer could verify successful delivery and cleanup from the process and desktop interaction, but the current CLI cannot independently measure compositor-side cursor displacement or scroll distance. Therefore the repeat-sampled small/medium/large delta A/B table and compositor acceleration/scroll reinterpretation measurement remain a mandatory calibration gate before M10 enables any real takeover default. This does **not** block M7–M9, which are strictly offline and use only traces/synthetic `ContactFrame`s plus recording/fake sinks.

M7 may start under its offline-only safety constraints. M10 remains forbidden until the remaining A/B measurements are recorded and the output backend is explicitly qualified for takeover use.
