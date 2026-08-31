# M9 Execution Task — Two-Finger 2D Scroll and Secondary Click (Offline)

Status: ready after M8 approval in `reviews/M8_REVIEW.md` re-review 2.

Implement M9 only. Read `design.md` §7/§9/§10/§13, `PHASE2_PLAN.md` M9, `DESIGN_V2.md` §18–§19, `M7_TASK.md`, `M8_TASK.md`, `reviews/M7_REVIEW.md`, `reviews/M8_REVIEW.md`, this file, and the current arbiter/output/decoder tests before editing.

## Objective

Extend the single platform-independent `touchpad-core` Interaction Arbiter with configurable:

1. exactly-two-finger, two-dimensional pixel scrolling;
2. explicit natural/non-natural direction;
3. a complete semantic `ScrollBegin → ScrollDelta* → ScrollEnd` lifecycle;
4. two-finger tap mapped to one secondary-button click pair;
5. buttonpad physical click while exactly two valid fingers are down mapped to a latched secondary-button press/release when that policy is enabled.

M9 remains pure offline policy. It must not connect to live input or the M6 output backend. It establishes semantic behavior for the later M10 vertical slice; it does not qualify real desktop scrolling.

## Configuration and compatibility

- Preserve M8 behavior by default. Existing `ArbiterConfig::new(...)` and existing configurations must leave M9 two-finger policy disabled unless an explicit validated two-finger configuration is supplied.
- Add typed public configuration sufficient for: scroll enabled, natural direction, linear scroll logical-pixels-per-mm scale, scroll commit threshold in millimetres, secondary tap enabled, two-finger physical-click-to-secondary enabled, maximum secondary-tap duration, and maximum per-contact secondary-tap movement.
- Reject non-finite/non-positive scales and distances, zero durations, and impossible feature combinations. Do not silently coerce values.
- KDE/libinput settings are A/B references only. Do not read KDE configuration or copy its values into hidden defaults.
- Natural-direction sign must be explicit and tested. For the core semantic contract, `natural=true` means output scroll delta has the same sign as the two-finger centroid movement on each axis (content follows fingers); `natural=false` negates each axis. M10/M12 may later calibrate backend convention, but M9 must not leave sign implicit.

## Single arbiter and ownership

- Keep one `Arbiter` as the only policy owner. Do not create independent pointer/tap/scroll recognizers that can commit against the same frame.
- Add a typed observable two-finger phase sufficient to distinguish at least idle, candidate, committed scrolling, physical-secondary-click-held, cancelled, and finished. Include it in `FrameDecision` and expose it from `Arbiter` without breaking existing serde compatibility unnecessarily.
- Exactly two complete live contacts may form a two-finger candidate. The frame where the second valid contact appears anchors the interaction; no pointer, button, or scroll event may leak during the candidate period.
- Entering the two-finger family cancels/finishes incompatible M7/M8 one-finger ownership deterministically. A sticky synthetic-left drag lock must be released according to M8's aggregate-source rules before two-finger policy can own the contacts.
- Once scrolling commits, that interaction cannot also produce a primary/secondary tap or pointer movement. Once a physical secondary click owns the button press, the same contact interaction cannot commit scroll/tap output.
- When finger count drops below or rises above exactly two, the current two-finger interaction ends/cancels. A remaining `Active` contact must not silently become a one-finger pointer/tap candidate without a genuine new `Began` boundary.

## Geometry and scroll semantics

- Identify the two contacts by tracking id, independent of slot/vector order. Tracking-id replacement, duplicate identity, or an unknown `Active` contact must not reuse old anchors/remainders.
- Track an anchor/current position for each contact and the two-finger centroid. Track maximum displacement of **each contact from its own anchor**, not only centroid motion, so opposing pinch/rotate-like motion cannot return and qualify as a secondary tap.
- Scroll commit is based on centroid displacement from the candidate centroid anchor. Equality at the configured threshold commits; strictly below remains candidate.
- On commit emit `ScrollBegin`, then the accepted accumulated centroid displacement exactly once as `ScrollDelta` when quantization yields a non-zero axis. Thereafter emit incremental deltas. Preserve per-axis sub-pixel remainder and reset it at every finish/cancel/release/new interaction.
- Horizontal, vertical, and diagonal motion are first-class. A diagonal delta must preserve both non-zero axes; do not collapse to a dominant axis or add axis lock in M9.
- `ScrollDelta` values are typed `LogicalPixels`. Zero/zero deltas produce no `ScrollDelta`. `ScrollBegin` is emitted exactly once per committed lifecycle and `ScrollEnd` exactly once when it ends.
- If a committed scroll loses one finger, gains a third finger, receives missing required coordinates, suffers tracking replacement, or is deterministically cancelled/discontinuous on a valid frame, emit `ScrollEnd` before leaving the scroll phase. No scroll event may appear before `ScrollBegin` or after `ScrollEnd`.
- A `discontinuity=true` frame may re-anchor a two-finger candidate for future relative scroll, but contacts seeded across that boundary are ineligible for secondary tap because their real down time and prior movement are unknown.
- Use only `ContactFrame.monotonic_timestamp` and checked duration arithmetic. Boundary equality for duration/movement/threshold is accepted; strictly greater duration/movement disqualifies tap.

## Two-finger secondary tap

- A two-finger interaction is a secondary tap only if secondary tap is enabled, both initial contacts were valid, no scroll committed, duration is within the limit, each contact's maximum anchor displacement is within the limit, no third contact/physical click/discontinuity/error competed, and the interaction ends by dropping below two fingers.
- Emit exactly `ButtonDown(Right), ButtonUp(Right)` in order at the qualifying two-finger release boundary. Do not delay it and do not invent a secondary-click event.
- If the two fingers lift on different frames, fire at most once at the first boundary that ends the exactly-two interaction; the remaining old `Active`/`Ended` contact cannot generate primary pointer/tap output.
- Too-long, too-far, opposing-motion, incomplete, cancelled, discontinuous, or scroll-committed sequences emit no secondary click.
- Two-finger double-tap Smart Zoom is not M9; two qualifying secondary taps remain two ordinary right-click pairs.

## Buttonpad physical two-finger click

- When the physical left source transitions up→down while exactly two complete valid fingers are present and the configured two-finger physical-click policy is enabled, map that physical press to `ButtonDown(Right)` instead of `ButtonDown(Left)`.
- Latch the chosen owner for the entire physical press. Finger-count/contact changes while held must never remap Right back to Left (or vice versa). The matching physical release emits exactly one `ButtonUp` for the latched button.
- A physical-left press that began before the second finger appeared remains a primary-left press. Existing M7/M8 physical/synthetic left aggregation and drag-lock cleanup must remain correct.
- A two-finger physical click cancels the secondary-tap/scroll candidate and emits no synthetic secondary tap on release.
- Centralize secondary-button source arbitration. A synthetic right tap pulse and a latched physical right press must not produce duplicate downs, unmatched ups, or a stuck button. Actual `physical_buttons.right` behavior must be preserved or explicitly handled without weakening the existing left contract; do not silently alias simultaneous sources.
- M9 does not implement geometric bottom-right click zones, middle-button emulation, or configurable desktop shortcuts. The M9 click strategy is exactly-two-finger buttonpad click plus exactly-two-finger tap.

## Failure, cleanup, and accepted-prefix delivery

- Preserve `ContactFrame::validate()` and atomic draft commit. Error/Fatal frames change no pointer/tap/scroll/button/timing/baseline state and emit nothing.
- Sequence/timestamp regression remains fail-closed. If no decision can be returned, any delivered/owed open scroll or held button must remain visible to cleanup.
- Extend pure `Arbiter::release_all` so it deterministically emits any required `ScrollEnd` and right/left button releases exactly once, then resets all M7–M9 phases, anchors, remainders, disqualification flags, button owners, and regression baselines. Repeated cleanup is empty.
- Extend `ArbiterSink`'s accepted-prefix/fail-stop model beyond delivered left: acknowledge held Right and open scroll only after the sink accepts the relevant event. A rejected `ScrollBegin` owes no `ScrollEnd`; an accepted begin followed by rejected delta/end remains open and cleanup must close it. A rejected right down owes no up; an accepted right down followed by rejected up remains held and cleanup must release it.
- Wrapped `OutputSink::release_all` remains the authoritative cleanup acknowledgement. Explicit cleanup plus wrapped cleanup must be idempotent/retryable with no duplicate/unmatched button up or scroll end. Normal frames remain blocked while faulted.
- Deterministic event ordering: physical/synthetic button down before any owned motion; `ScrollBegin` before first delta; final scroll delta before `ScrollEnd`; a secondary tap is down then up with no interleaved pointer/scroll output.
- Preserve all 647 existing tests and all M1–M8 public behavior, source-aware left arbitration, checked timing, serde/finite-unit guarantees, and `#![forbid(unsafe_code)]`. No new dependency is expected.

## Required offline tests

Cover at least:

- disabled defaults, invalid configuration, and natural/non-natural sign on both axes;
- two-finger candidate with no leakage; exact threshold below/equal/above; accumulated first delta exactly once; continued x-only/y-only/diagonal scroll; negative direction; sub-pixel many-small-vs-aggregate and remainder reset;
- contact vector/slot order changes with stable tracking ids; one finger lifts; both lift same frame; third finger; missing coordinates; tracking replacement; remaining `Active` cannot become pointer;
- secondary tap duration and per-contact displacement below/equal/above; anchor-return and opposing movement do not falsely tap; staggered lift fires once; disabled tap produces nothing; two secondary taps produce two right click pairs;
- scroll wins over secondary tap; two-finger family wins over one-finger pointer/tap without double commit; entry while tap follow-up/drag lock is active releases/cancels correctly;
- physical two-finger click maps to latched Right down/up; one-finger physical click remains Left; press-before-second-finger remains Left; finger-count changes while pressed do not remap; physical/synthetic right overlap truth table;
- discontinuity re-anchor scroll but no secondary tap; invalid-frame atomicity; timestamp/sequence regression; `release_all` during candidate/scroll/right-held and fresh interaction after reset;
- `ArbiterSink` fault injection at rejected scroll begin, rejected first delta after accepted begin, rejected scroll end, rejected right down, rejected right up after accepted down, and cleanup failure/retry with exact accepted logs;
- synthetic frames and replay-derived frames use the same arbiter path, with at least one diagonal scroll fixture and one secondary-tap fixture.

Use deterministic timestamps. Do not sleep in tests.

## Hard scope limits

- Do not open, enumerate, read, record, or grab `/dev/input`.
- Do not call Portal, EIS, libei, `output-probe --emit`, desktop automation, or any real output sink.
- Do not add CLI takeover, daemon, autostart, privileges, environment changes, or live calibration.
- Do not implement momentum/inertia (M12), scroll acceleration/filtering/axis lock (M12), pinch/rotate/swipes/Smart Zoom/page navigation/edge gestures (M14+), palm/thumb classification (M13), Force Click, pressure, or haptics.
- Do not edit `design.md`. Do not commit or push. Never write credentials.

## Deliverables and gates

- Implement code plus focused unit/public/replay integration tests.
- Update `DESIGN_V2.md` and `MILESTONES.md` with honest M8 approval and M9 implementation facts/limitations; do not claim M9 approved before review.
- Run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Report exact changed files, implemented/not-implemented behavior, exact test totals, automated versus live validation, deviations/dependencies/unsafe, and reviewer risks. Stop after M9.
