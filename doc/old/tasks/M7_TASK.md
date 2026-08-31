# M7 Execution Task — Offline Arbiter, One-Finger Pointer, Physical Click

You are the implementation agent. Implement **M7 only** from `PHASE2_PLAN.md`.

Before editing, read in full:

- `design.md`
- `IMPLEMENTATION_BRIEF.md`
- `DESIGN_V2.md`
- `MILESTONES.md`
- `PHASE2_PLAN.md`
- `reviews/M6_REVIEW.md`
- this task file
- the current workspace source and tests

## Milestone objective

Add the platform-independent, deterministic offline interaction layer that consumes normalized `ContactFrame`s and produces typed semantic `OutputEvent`s for:

1. unified interaction lifecycle (`Candidate`, `Committed`, `Cancelled`, `Finished`);
2. one-finger relative pointer motion through an explicitly configured linear millimetre-to-logical-pixel mapping;
3. physical left-button down/up, ordinary click sequences, repeated click sequences (desktop double-click semantics), and button-held pointer dragging.

M7 is an offline policy milestone. It must not connect to the M6 real backend and must not touch live input.

## Required architecture and behavior

### 1. One arbiter owns competition

- Put platform-independent policy in `touchpad-core`; it must not depend on Linux, evdev, KDE, Wayland, portal, libei, or desktop crates.
- All contacts enter one Interaction Arbiter. Do not create independent pointer/tap/scroll recognizers that can each commit against the same frame.
- Encode observable lifecycle transitions for `Candidate`, `Committed`, `Cancelled`, and `Finished`; illegal transitions return structured errors or deterministic diagnostics, never panic.
- Keep the model extensible for M8 tap/tap-drag and M9 scroll, but do not implement those behaviors now.
- At most one mutually exclusive contact interaction is committed at a time. A second live contact, discontinuity, invalid/missing required coordinates, timestamp regression, or sequence regression must deterministically cancel the one-finger candidate/interaction and emit no further pointer movement from it.

### 2. Candidate period must not leak output

- A new one-finger contact starts as a pointer/tap-family candidate. Before its explicit configurable motion threshold is crossed, emit no `PointerMove` and no synthetic button event.
- On pointer commitment, cancel conflicting future candidates as a single arbiter transition. The first committed movement must account exactly once for the accepted displacement accumulated since the candidate anchor; do not lose it or emit it twice.
- A contact that begins and ends below threshold produces no event in M7; M8 will add tap semantics.
- Zero movement produces no `PointerMove`.

### 3. Linear pointer mapping and units

- Require an explicit validated M7 configuration for motion threshold and linear `logical_pixels_per_mm`; do not silently read KDE settings and do not claim a macOS acceleration curve.
- Preserve type separation: contact positions/deltas remain `Millimeters`; semantic output remains `LogicalPixels`; raw counts never enter this layer.
- Use checked finite arithmetic. Overflow/non-finite configuration or results must fail closed with structured errors and no partial batch.
- Maintain per-axis fractional/remainder state where conversion/quantization requires it; reset it on cancellation/discontinuity and prevent residue from one contact leaking into another. Document the exact invariant and test many small deltas versus one equivalent aggregate delta.
- M11, not M7, owns acceleration, jitter filtering, velocity estimation, tracking-speed tuning, and measured macOS-like curves.

### 4. Physical left-button lifecycle and dragging

- Consume `ContactFrame.physical_buttons.left` edges atomically with the frame. Emit exactly one `ButtonDown(Left)` on false→true and exactly one `ButtonUp(Left)` on true→false; stable state emits nothing.
- Repeated down/up pairs pass through in timestamp/frame order without artificial delay or invented desktop events. Two valid pairs are the physical double-click representation.
- While left is held and one-finger pointer motion is committed, emitted movement represents a physical drag. Define deterministic same-frame ordering: press precedes movement that belongs to the drag; final movement precedes release.
- Physical-button release must never be suppressed by contact cancellation, added fingers, missing touch coordinates, or discontinuity. If the arbiter previously emitted a down, its shutdown/reset path must produce the matching up exactly once and be idempotent.
- Do not implement tap-to-click, tap-and-drag, drag lock, right/middle mapping, Force Click, or pressure behavior in M7.

### 5. Pure offline API and failure atomicity

- Drive the implementation only with synthetic/trace-derived `ContactFrame`s. Prefer a pure frame-decision result containing ordered events and lifecycle transitions; tests may feed it to `RecordingSink`/a fault-injecting fake, but production M7 code must not instantiate `PortalDesktopOutput`.
- Validate a frame and all arithmetic before committing internal state or returning an event batch. A rejected frame must not leave half-applied contact, button, scale, remainder, or lifecycle state.
- Provide an idempotent `cancel_all`/`release_all`-equivalent semantic path suitable for later M10 shutdown. It must release a logically held left button and clear candidates/residue even after prior errors.
- No `unsafe` is expected or permitted in new M7 core code.

## Required deterministic coverage

Add focused unit/integration tests for at least:

- legal and illegal lifecycle transitions through all four states;
- one-finger begin/active/end below threshold (no output);
- exact threshold boundary and just-over-threshold commitment;
- accumulated first committed delta exactly once, then incremental deltas;
- horizontal, vertical, diagonal, negative, zero, and many-small-delta motion;
- tracking-id replacement and slot reuse without stale anchor/remainder leakage;
- second contact before commitment and after commitment;
- missing coordinate, duplicate slot/invalid frame, discontinuity, time regression, and sequence regression fail/cancel behavior;
- physical down/up, stable-state deduplication, click, two click pairs, press-without-contact, and release despite cancellation/discontinuity;
- deterministic press/move/release ordering and button-held drag;
- shutdown while button held, repeated shutdown, and reset followed by a fresh interaction;
- output/fake-sink failure does not corrupt arbiter state if a sink adapter is provided;
- trace/replay-derived frames and directly synthetic frames exercise the same arbiter path.

Use property-style/table-driven tests where they materially cover state transitions and unit invariants, but do not add a large dependency without justification and `THIRD_PARTY.md` updates.

## Hard safety and scope limits

- Do not open, enumerate, read, record, or grab `/dev/input`.
- Do not invoke `output-probe --emit`, Portal, EIS, libei, desktop automation, or any real `OutputSink`.
- Do not add CLI takeover, daemon/service/autostart behavior, privileges, uinput, or environment configuration changes.
- Do not implement tap, tap-and-drag, drag lock, two-finger scroll/right-click, momentum, gestures, acceleration curves, palm/thumb classification, or haptics.
- Preserve M1–M6 behavior and all R1–R13 repairs.
- Never write credentials to files or output. Do not commit or push.

## Acceptance gates

Run and report:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Also run targeted M7 offline tests explicitly. Do not run any live input or output command.

End with the six-part review handoff required by `PHASE2_PLAN.md`: exact changed files; implemented/not implemented; exact gate totals; separation of automated/probe/live validation; deviations/dependencies/unsafe; reviewer risks. Stop after M7.
