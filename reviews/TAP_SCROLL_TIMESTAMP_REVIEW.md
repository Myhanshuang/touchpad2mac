# Tap / Scroll / Timestamp Refactor Review

## Scope

This review covers the post-M19 input-policy refactor prompted by direct KDE
6.7.4 / libinput behavior comparison:

- one-finger tap-and-drag commit semantics;
- tap-family arbitration against physical buttons, contact replacement,
  discontinuity and competing multi-finger ownership;
- the ownership layer of two-finger kinetic scrolling;
- propagation of source monotonic timestamps to the desktop output backend.

## Tap-and-drag: deferred release

The previous policy completed the first tap as `ButtonDown + ButtonUp` and then
kept a follow-up window in which another contact could synthesize a fresh
`ButtonDown` for drag. That allowed the first click to mutate desktop state
(for example minimizing a window) before the follow-up drag decision.

The new state machine follows libinput's deferred-commit model:

```text
first qualifying tap release
    -> ButtonDown(Left)
    -> FollowUpWindow (press remains held)

timeout
    -> ButtonUp(Left)
    -> click completed

follow-up contact + committed motion
    -> reuse held left
    -> PointerMove
    -> TapDragContact

clean second tap
    -> ButtonUp(Left)
    -> ButtonDown(Left)
    -> fresh FollowUpWindow
```

There is no extra `double_tap_before_drag` product rule. The temporary
workaround and its tap-chain counter were removed.

M19 keeps `max_tap_drag_gap = 180 ms` and disables one-finger sticky drag
lock. A committed M19 tap-drag therefore releases left on its clean lift.

## Tap arbitration

The synthetic left source remains part of the same aggregate button ownership
model as physical input.

- physical left may take over an already-held synthetic left without emitting
  a duplicate down or creating a wire gap;
- physical right, extra fingers, tracking replacement and discontinuity
  resolve/cancel the pending tap-family ownership;
- cancellation always leaves the aggregate wire state consistent with the
  post-frame physical/synthetic sources;
- timeout is driven through the existing monotonic policy tick, so a pending
  click completes even when no later evdev frame arrives;
- empty semantic decisions are no longer submitted to the output sink.

The existing palm/thumb robustness filter remains upstream of arbitration:
suppressed contacts do not become tap owners, while retained contact metadata
continues through the same single Arbiter.

## Scroll lifecycle boundary

The live Arbiter no longer produces kinetic `ScrollDelta` events after the
fingers leave the touchpad. A committed two-finger finger scroll now ends on
the clean release frame:

```text
ScrollBegin
ScrollDelta ...
finger release
ScrollEnd
```

This mirrors libinput's layering decision: the input layer does not know the
target widget/surface after contacts disappear, so kinetic continuation is a
higher-level responsibility.

The old momentum configuration fields and pure momentum math remain in the
schema/module for compatibility and migration stability, but the current
takeover Arbiter does not invoke them after finger release. The settings UI
marks these three fields as compatibility-only:

- `scroll.momentum_tau_ms`
- `scroll.momentum_start_speed_mm_per_s`
- `scroll.momentum_stop_speed_mm_per_s`

## Source timestamp propagation

`OutputSink` now has a backwards-compatible timestamped frame entry point:

```text
ContactFrame.monotonic_timestamp
    -> ArbiterSink::submit_frame_at
    -> streaming output wrappers
    -> PortalOutputSink
    -> Transport::frame_at(source_time_us)
    -> NativeTransport
    -> ei_device_frame(source_time_us)
```

Legacy sinks/transports inherit the old behavior through default methods.
Native libei output converts source nanoseconds to microseconds and no longer
has to stamp source-driven frames with `ei_now()`.

### Deliberate phase-1 limitation

This is frame-level source timing, not full libinput historical per-event
backdating. Events resolved later by a timer (for example the deferred tap
`ButtonUp`) are currently stamped with the policy-tick timestamp. Multiple
semantic events from one input frame share that frame's source timestamp.

Reaching full libinput semantics would require event-level physical timestamps
separate from recognition/decision time. That is a future extension rather
than being falsely implied by this refactor.

## Regression coverage

The refactor updates both private and public contracts for:

- deferred single-tap release;
- follow-up drag reusing the original held press;
- multi-tap `Up -> Down` transition;
- drag-lock and final-frame commitment;
- physical-button takeover and release;
- multi-finger, discontinuity and tracking-replacement cancellation;
- near-`u64::MAX` timeout arithmetic;
- partial output failure and cleanup while a deferred press is held;
- M11 fidelity preserving tap ownership before pointer scaling;
- M12 clean finger release producing `ScrollEnd` with inert post-release
  policy ticks;
- Arbiter source timestamp forwarding;
- Portal/fake transport source timestamp conversion;
- NativeTransport using the supplied `frame_at` timestamp.

## Review verdict

Architecturally the new split is preferable to the previous workaround:

- tap ambiguity is resolved before releasing the externally visible click;
- kinetic scroll ownership no longer lives below the layer that can know its
  target;
- output timing can preserve the input clock domain without forcing every
  backend to migrate at once.

**APPROVED.** The refactor passed the complete workspace quality gate:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings PASS
cargo test --workspace --locked                                PASS
cargo test --release --workspace --locked                      PASS
cargo build --release -p touchpadctl --locked                  PASS
git diff --check                                                PASS
```

The remaining live-validation item is experiential rather than a code-review
blocker: compare the deferred tap behavior and finger-scroll termination on
the real touchpad/KWin path, especially a tap on a minimize control followed
by an immediate new contact and motion.
