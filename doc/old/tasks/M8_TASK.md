# M8 Execution Task — Tap, Tap-and-Drag, Sticky Drag Lock (Offline)

Status: ready after M7 approval in `reviews/M7_REVIEW.md` re-review 2.

Implement M8 only. Read `design.md` §7/§10, `PHASE2_PLAN.md` M8, `DESIGN_V2.md` §18, `M7_TASK.md`, `reviews/M7_REVIEW.md`, this file, and the current arbiter/tests before editing.

## Objective

Extend the platform-independent `touchpad-core` Interaction Arbiter with configurable one-finger:

1. tap-to-left-click;
2. two consecutive taps represented as two correctly timed left click pairs (desktop double-click semantics; no invented double-click event);
3. tap followed promptly by another one-finger touch and movement as tap-and-drag;
4. sticky drag lock: after a real tap-drag movement, lifting may keep left held, another one-finger contact may continue the drag, and an additional qualifying tap ends the lock.

M8 is pure offline policy. It must not connect to live input or the M6 output backend.

## Configuration and time

- Preserve M7 behavior by default: existing `ArbiterConfig::new(...)` must leave tapping disabled unless an explicit validated tap configuration is supplied.
- Provide typed public configuration for: tap enabled, tap-and-drag enabled, sticky drag lock enabled, maximum tap duration, maximum first-contact movement for a tap, and maximum gap from a completed tap to the next touch that may begin tap-and-drag.
- Reject zero/invalid durations, non-finite or non-positive movement thresholds, and impossible feature combinations (tap-and-drag requires tap; drag lock requires tap-and-drag). Do not silently coerce them.
- Use only `ContactFrame.monotonic_timestamp` and checked duration arithmetic. Never read wall clock or a process-local clock.
- Boundary policy must be documented and tested: equality at configured duration/distance/gap is accepted; strictly greater expires/cancels.
- Timeouts are evaluated at incoming frame boundaries. Sticky drag lock has no autonomous timeout in M8; `release_all` is the unconditional escape path.
- Existing KDE/libinput values are A/B reference only. Do not read KDE configuration, copy hidden defaults, or depend on libinput at runtime.

## Single arbiter and observable state

- Keep one Arbiter as the only policy owner. Do not add independent recognizers that can both commit against a frame.
- Add a typed, observable tap/drag phase (names may vary) sufficient to distinguish at least: idle, first-tap candidate, follow-up window, tap-drag contact, locked-without-contact, locked-contact continuation, and cancelled/finished outcomes.
- Preserve M7 `Candidate/Committed/Cancelled/Finished` pointer lifecycle and its first-delta/remainder invariants. Refactor only where M8 ownership requires it.
- A tap candidate tracks maximum displacement from its own anchor, not merely the last delta. Crossing the tap threshold permanently makes that contact ineligible for tap, even if it returns to the anchor.
- Pointer commitment wins once the M7 motion threshold is crossed. A contact must never produce both ordinary pointer output and a tap click.

## Tap semantics

- A one-finger Began→Ended contact is a tap only when tapping is enabled, required coordinates are valid, duration is within the limit, maximum displacement is within the tap limit, no extra live contact appeared, no physical click competed, and no discontinuity/cancellation/error occurred.
- Emit the tap at the qualifying release frame as exactly `ButtonDown(Left), ButtonUp(Left)` in order.
- Two qualifying taps naturally emit two click pairs in frame/timestamp order. Do not delay the first click and do not add a special double-click output event.
- A too-long, too-far, incomplete, cancelled, multi-contact, or discontinuous sequence emits no synthetic click.
- When tapping is disabled, all M7 below-threshold sequences remain output-free.

## Tap-and-drag semantics

- A qualifying first tap opens the configured follow-up window.
- If exactly one new valid finger begins at or before that deadline and tap-and-drag is enabled, enter a **pending follow-up candidate with no synthetic button held**. This is the current safety contract: a re-touch/tracking-id bounce must not create a held left button merely because it began inside the follow-up window.
- Motion on the follow-up contact uses M7's typed linear mapping, threshold, accumulated first delta, and remainder rules. Only when pointer motion commits does the arbiter emit one logical left down, immediately followed by the first committed pointer delta, and mark the interaction as a real drag.
- If the follow-up contact ends cleanly without committed drag motion and still qualifies as the second tap, emit its complete `ButtonDown(Left), ButtonUp(Left)` pulse at release. Otherwise emit no synthetic button event. An uncommitted follow-up never enters drag lock.
- Tracking-id replacement, cancellation, missing coordinates, or competing ownership while the follow-up is still pending must not synthesize a left down.
- If committed drag motion occurred and sticky drag lock is disabled, final motion precedes one synthetic up on finger release.
- If committed drag motion occurred and sticky drag lock is enabled, finger release keeps synthetic left held and enters locked-without-contact.
- A follow-up arriving strictly after the window is an ordinary new pointer/tap candidate and must not synthesize an early down.

## Sticky drag-lock semantics

- In locked-without-contact, one valid new finger begins a locked-contact candidate without another button down.
- If that contact crosses the M7 pointer threshold, emit the accumulated first delta once, continue normal drag motion, and return to locked-without-contact when lifted, still without an up.
- If that contact ends as a qualifying tap without committed movement, emit exactly one logical left up and leave drag lock.
- A non-qualifying long/too-far contact that never commits motion must not fabricate a click; it may leave the lock held for another continuation attempt. `release_all` always ends it.
- Extra live contacts, discontinuity, invalid active coordinates, or deterministic cancellation end synthetic drag/lock fail-closed with one logical up unless the physical source still holds left.

## Physical/synthetic left-button arbitration

Refactor the current single `held_left` assumption into source-aware policy while preserving public output behavior:

- track physical-left state and synthetic-left state separately;
- expose only their logical OR to the output sink;
- emit `ButtonDown(Left)` only on aggregate false→true and `ButtonUp(Left)` only on aggregate true→false;
- a physical press cancels pending tap/follow-up policy and wins over synthetic click generation;
- if physical left becomes held during a synthetic drag/lock, do not emit a duplicate down; ending the synthetic source must not emit up until physical left is released;
- if synthetic drag begins while physical left is held, do not emit a duplicate down;
- preserve deterministic ordering: any aggregate down precedes drag motion; final motion precedes an aggregate up;
- physical release must remain observable despite tap cancellation, extra contacts, missing coordinates, or discontinuity.

The same-frame synthetic tap pulse must still produce down then up even though aggregate state begins and ends false. Centralize this button multiplexing so physical and synthetic paths cannot independently emit contradictory events.

## Failure, cleanup, and compatibility

- Preserve frame validation and atomic draft commit: rejected Error/Fatal frames change no pointer, tap, timing, button-source, baseline, or output state.
- Sequence/timestamp regression remains fail-closed. Any synthetic held state must remain visible to `release_all`, which emits the required aggregate up exactly once and resets every pointer/tap/drag/lock/timing/source state.
- Preserve the M7 `ArbiterSink` accepted-prefix/fail-stop contract for synthetic click and drag events. Add stateful sink fault tests for rejected tap down, rejected tap up after accepted down, motion failure after accepted synthetic down, and cleanup/recovery while drag-locked. No unmatched/duplicate up or lost release is allowed.
- Preserve all M1–M7 APIs where reasonably possible, all 574 existing tests, M7 pointer/physical-click behavior, serde/finite-unit guarantees, and `#![forbid(unsafe_code)]`.
- No new dependency is expected. Any unavoidable dependency requires license documentation and justification.

## Required offline tests

Cover at least:

- disabled defaults and invalid configuration combinations;
- duration, distance, and follow-up gap: below/equal/above boundaries;
- anchor-return path cannot become a tap after exceeding maximum displacement;
- single tap; two tap click pairs; long/large/cancelled tap produces nothing;
- first tap then timely second touch: no-motion second click, drag threshold crossing, first accumulated drag delta once, continued drag, final ordering;
- follow-up window expiry;
- drag lock disabled release; sticky lock lift/reposition/continue; repeated reposition; qualifying tap unlock; non-qualifying unlock attempt; `release_all` while locked;
- second finger before tap, during tap-drag, and during lock;
- physical press/release competition in tap candidate, synthetic drag, and locked states, including aggregate OR truth table and same-frame ordering;
- discontinuity, missing active coordinates, tracking replacement, invalid frame atomicity, timestamp/sequence regression, and reset followed by a fresh interaction;
- `ArbiterSink` partial failures for synthetic events and exact recovery logs;
- synthetic frames and replay-derived frames take the same arbiter path.

Use deterministic synthetic timestamps. Do not sleep in tests.

## Hard scope limits

- Do not open, enumerate, read, record, or grab `/dev/input`.
- Do not call Portal, EIS, libei, `output-probe --emit`, desktop automation, or any real output sink.
- Do not add CLI takeover, daemon, autostart, privileges, or environment changes.
- Do not implement M9 two-finger tap/right-click/scroll, momentum, pinch/rotate/swipes, acceleration curves, palm/thumb classification, Force Click, pressure, or haptics.
- Do not commit or push. Never write credentials.

## Deliverables and gates

- Implement code and focused unit/public/replay integration tests.
- Update `DESIGN_V2.md` and `MILESTONES.md` with honest M8 implementation facts and limitations; do not edit `design.md`.
- Run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Report exact changed files, implemented/not implemented behavior, exact test totals, automated versus live validation, deviations/dependencies/unsafe, and reviewer risks. Stop after M8.
