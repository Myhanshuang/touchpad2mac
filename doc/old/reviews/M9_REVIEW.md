# M9 Review — Two-Finger 2D Scroll and Secondary Click (Offline)

Date: 2026-08-17  
Decision: **REJECTED — repair M9; do not start M10**

The typed configuration, centroid/identity tracking, 2D natural-direction scroll lifecycle, per-contact tap displacement, right-source multiplexer, latched buttonpad click, accepted-prefix extension, offline replay fixtures, and broad deterministic suite are a strong base. Independent formatting, clippy, and all 708 tests pass. The passing suite misses several ownership and feature-gating paths that can emit explicitly disabled or contradictory desktop semantics.

## Blocking findings

### R1 — Critical: `scroll_enabled=false` is ignored by the scroll commit path

`TwoFingerConfig` stores and exposes `scroll_enabled`, but `update_two_finger_pair` commits whenever centroid displacement reaches the threshold without checking that flag. Supplying a configuration only to enable secondary tap or two-finger physical click can therefore still emit `ScrollBegin` and `ScrollDelta` even though scrolling was explicitly disabled.

Gate candidate creation/commit consistently by capability. Add public and unit regressions with `scroll_enabled=false` and the other two capabilities independently enabled/disabled; motion past the threshold must never open or emit a scroll lifecycle. A disabled capability must not become active merely because an `Option<TwoFingerConfig>` exists.

### R2 — Critical: prior/held one-finger ownership can be followed by a synthetic Right tap in the same contact cluster

Two related paths fail to disqualify the new secondary-tap candidate:

1. A primary physical-left press begins with one finger. While Left remains held, the second finger appears. The two-finger candidate anchors because the physical down edge is no longer new. `secondary_tap_qualifies` checks physical Right/latch but not physical Left. Dropping below two fingers can therefore emit `ButtonDown(Right), ButtonUp(Right)` while Left is still held.
2. A one-finger pointer interaction commits and emits `PointerMove`; before that original finger lifts, a second finger appears. The one-finger lifecycle is cancelled and a tap-eligible two-finger candidate anchors. A quick small lift can then emit a secondary click, so one continuous contact cluster commits pointer and secondary-tap ownership.

Physical button ownership and already-committed pointer ownership must permanently disqualify secondary tap for the continuing contact cluster. Scrolling may re-anchor only if its capability is enabled and the button-ownership policy explicitly permits it; no secondary tap may fire until all competing contacts end and a genuinely fresh cluster begins. Add exact event tests for both sequences, including the physical Left still held at the release boundary and pointer commit followed by a quick two-finger release.

### R3 — High: cancellation clears secondary-tap disqualification too early

`end_two_finger(...Cancel...)` calls `clear_two_finger_interaction`, which resets `two_tap_disqualified=false`. After a third finger, missing coordinates, tracking replacement, or a regression cancels the interaction, two surviving `Active` contacts can stabilize on a later frame, form a fresh tap-eligible candidate, and emit a Right click. That contradicts the secondary-tap contract: a third contact, invalid coordinates, replacement, discontinuity, regression, or other deterministic cancellation in the current contact cluster makes it ineligible for tap.

Separate per-candidate geometry reset from contact-cluster tap eligibility. Cancellation must retain disqualification until the affected cluster fully drains (or a clearly defined genuinely new `Began` cluster starts); it may still allow a safe relative-scroll re-anchor where specified. Cover at least third-finger → back to the original two Active contacts, missing-coordinates → valid Active recovery, tracking replacement → stable pair, and timestamp/sequence regression → later monotonic frame. None may synthesize a secondary tap; after all contacts end, a genuinely fresh pair must work normally.

### R4 — High: cross-family handoff ordering briefly overlaps incompatible ownership

The final assembler globally places every button down before policy events and every button up after them. That is correct for a single drag/tap lifecycle, but wrong for ownership handoff:

- a physical Right press while scrolling produces `[ButtonDown(Right), ScrollEnd]`, so the new click becomes held before the old scroll lifecycle closes;
- if sticky synthetic Left is held and a two-finger physical click arrives on the same frame that establishes the pair, output is `[ButtonDown(Right), ButtonUp(Left)]`, briefly creating a Left+Right chord even though M9 requires the old drag lock to release before two-finger click ownership begins.

Represent ordered intents (or separate pre-handoff releases from current-owner downs) instead of globally bucketing all buttons. Required ordering is: final delta, then `ScrollEnd`, then the new physical-button down; and old synthetic Left up before a newly latched Right down. Preserve M8's within-owner invariants (down before drag movement; final movement before matching up) and add exact same-frame event regressions in debug and release profiles.

### R5 — Medium: multiple explicit cleanup failures are structurally collapsed to one error

M9 allows Right and Scroll to be owed simultaneously. `ArbiterSink::release_all` attempts both explicit releases, but `primary = primary.or(Some(err))` discards every explicit failure after the first. State remains retryable, but the returned structured error cannot report that both `ButtonUp(Right)` and `ScrollEnd` failed. This is lossy precisely where M10 requires the primary failure and all cleanup diagnostics to survive.

Extend cleanup error reporting compatibly so every failed explicit release is observable while preserving retry state and the wrapped-cleanup error. Add a dual-held test that identifies both failed events, not only `primary: Some(_)`, then proves the retry submits exactly the still-owed releases once.

### R6 — Medium: a secondary tap does not require a clean `Ended` record from the anchored pair

Dropping to one live contact is treated as `TwoEnd::Release` even when the missing member simply disappears from the frame and no complete `Ended` contact is present. Unlike the M8 tap path, M9 can then synthesize a click without a clean release record. Require release evidence for at least one anchored pair member at the first below-two boundary (and count its final coordinates toward displacement); otherwise cancel without a click. Add a disappearance-without-Ended regression and keep the staggered/both-Ended cases working.

## Independent verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 708 tests, 0 failed.
- Observed groups: `touchpad-core` unit 199, public M9 integration 11, Linux M9 replay integration 3.
- Credential-pattern scan outside generated/cache directories: 0 files.
- Scope scan matches the handoff: offline core/tests/two fixtures/docs only; no live input/output, `/dev/input`, grab, Portal/libei, or desktop emission was run.

## Repair scope

Repair R1–R6 and add exact public/unit regressions. Preserve M1–M8, correct M9 behavior, typed configuration, replay parity, source-aware Left/Right arbitration, and accepted-prefix cleanup. Keep all work offline. Do not start M10 or run live input/output commands.

---

## Re-review 1 — 2026-08-17

Decision: **REJECTED — R1–R6 are closed, repair R7 before M10**

The submitted repair closes R1–R6 structurally and independently passes formatting, warning-free clippy, and all 731 workspace tests. The new cleanup error shape preserves every explicit failure and retry state. One newly documented/tested ownership path is nevertheless incompatible with M9's single-arbiter contract.

### R7 — Critical: a held physical button can re-open two-finger scroll ownership

After a physical Left or Right down cancels a two-finger candidate/scroll, the next stable frame can call `begin_two_finger_candidate` while that raw physical button remains held. `secondary_tap_qualifies` blocks a synthetic Right tap, but `update_two_finger_pair` does not block scroll commit. Consequently a held primary or secondary click can coexist with a newly opened scroll lifecycle. For Right, the implementation and §20.9 explicitly use this invalid `physical right held + two-finger scroll` state to exercise dual cleanup errors.

Physical button ownership must exclude scroll ownership as well as secondary tap ownership. While aggregate physical Left or Right ownership is held, two-finger motion must emit no `ScrollBegin`/`ScrollDelta` and must not re-open a scroll after a press cancelled it. After the button is cleanly released, the same live contacts may establish a fresh relative scroll anchor (with secondary tap still disqualified for that contact cluster), or require a fresh contact cluster; document one deterministic policy and test it. Preserve the already-correct handoff order when a button press interrupts committed scroll: final delta if any, `ScrollEnd`, then the new button down.

Add exact unit and public regressions for both physical Left and physical Right: held before the pair forms; pressed during committed scroll; continued motion while held; and release/recovery behavior. Include an assertion that no frame exposes simultaneous physical-button and scroll ownership.

Do not retain an invalid ownership path solely to manufacture R5's dual-owed cleanup state. Test multiple explicit cleanup failures with a legitimate reachable state, such as simultaneous physical Left and Right holds producing two failed button-up releases; retain separate scroll cleanup/retry coverage. Correct `DESIGN_V2.md` and code comments that currently claim held Right plus scroll is valid.

### Re-review 1 verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 731 tests, 0 failed.
- R1–R6: closed by code inspection and their new regressions.
- Scope remains offline; no input grab or desktop output was run.

### Repair scope

Repair R7 only, update the R5 multi-failure regression to use a valid reachable state, and correct the corresponding design claims. Preserve R1–R6 and M1–M8. Keep the work offline and do not start M10.

---

## Re-review 2 — 2026-08-17

Decision: **APPROVED — M9 complete; M10 may begin**

R7 is closed. Physical Left, physical Right, and latched physical-secondary ownership now exclude candidate anchoring, discontinuity re-anchoring, and scroll commit. A clean button release permits the same still-live pair to establish a fresh relative-scroll anchor while the cluster remains secondary-tap-disqualified. The required interruption order remains `ScrollEnd` before the new button down, and tests assert after every relevant frame that physical-button ownership and an open scroll never coexist.

The R5 multiple-explicit-failure regression now uses the legitimate reachable state of simultaneous physical Left and Right holds. Independent inspection confirms both failed ups are reported, both remain retryable, and separate scroll cleanup tests retain accepted-prefix coverage.

### Final verification

- `cargo fmt --all -- --check`: pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass, 0 warnings.
- `cargo test --workspace --locked`: pass, 739 tests, 0 failed.
- `cargo test --release -p touchpad-core --locked`: pass, 278 tests, 0 failed, including all 23 public M9 tests.
- Credential-pattern scan outside generated/cache directories: 0 files.
- No live input, output, grab, Portal/libei, or system-setting operation was performed.

### M9 acceptance boundary

M9 is approved as a deterministic offline semantic layer. This approval does not qualify the live desktop output backend or authorize device takeover. M10 must retain explicit opt-in, bounded runtime, output/recorder readiness before grab, fail-open ordered shutdown, fake-backed failure-boundary tests, and a separate user-run live acceptance sequence.
