# M12 Acceptance — Scroll Fidelity and Momentum (`m12-scroll-v1`)

Status: **future user-run procedure; not executed during implementation or
review.** M12 code completion does not confer live qualification. M6 output
calibration plus the M10 and M11 acceptance prerequisites remain independent.

## Preconditions

- M6 relative-delta and pixel-scroll calibration evidence is recorded.
- M10 ordered 10/60/300-second `m10-linear-v1` acceptance is recorded.
- M11-specific acceptance is recorded before treating the combined pointer
  and scroll stack as live-qualified.
- Keep an external keyboard/mouse and a second terminal available.

## Bounded command

```text
touchpadctl takeover DEVICE TRACE \
  --takeover --confirm TAKEOVER --output-qualified \
  --profile m12-scroll-v1 --max-duration-seconds N
```

All M10 safety rules and the `1..=300` bound still apply. M12 adds no safety
flag and makes no macOS-equivalence claim.

## Staged observations

1. **30 s low/normal speed:** vertical, horizontal and diagonal pixel scroll;
   verify continuous gain with no wheel-step conversion or axis bias.
2. **30 s axis lock:** mostly-horizontal and mostly-vertical strokes should
   lock stably; gradual diagonal intent should release the lock without a
   snap or hysteresis trap.
3. **30 s reversal:** reverse direction repeatedly; no stale-velocity boost or
   momentum in the old direction may survive the reversal.
4. **60 s momentum:** make a fast two-finger stroke and lift cleanly. Scroll
   continues smoothly, decays monotonically and ends once. Slow releases must
   not start momentum.
5. **60 s cancellation matrix:** while momentum is active, test a new touch,
   physical click, reverse scroll, discontinuity/recovery and controlled
   SIGINT/SIGTERM. Momentum must cancel immediately and `ScrollEnd` must occur
   exactly once with no stuck scroll.
6. **60 s regression:** repeat M9/M10/M11 tap, secondary-click, buttonpad and
   pointer paths; enabling M12 must not alter their ownership semantics.

Record trace path, profile, duration, pass/fail, observed gain/axis-lock feel,
momentum duration, cancellation behavior and any deviation. If the desktop
converts pixel scroll to discrete wheel behavior, mark M12 live acceptance as
failed and keep the profile live-unqualified.
