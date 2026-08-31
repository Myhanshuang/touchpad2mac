# M14 Task — Continuous Gesture Recognition

Authority: `PHASE2_PLAN.md` M14. Implementation is offline/fake-backed.

## Goal

Add deterministic continuous recognition for pinch/zoom, rotate, two-finger
page swipe, Smart Zoom semantic action, edge gestures, three/four-finger swipe,
and thumb+three pinch/spread with explicit candidate/commit/cancel/progress.

## Contract

- Add platform-neutral continuous gesture types to the semantic output model;
  desktop backends that cannot emit them must return `Unavailable`, never fake
  native equivalence.
- Add a pure `gesture` recognizer with candidate/committed/cancelled states,
  monotonic progress, threshold hysteresis, direction reversal handling,
  contact-count change cancellation, and tracking-id stable geometry.
- Geometry derives from centroid, mean radius/span and pair angle in normalized
  millimeters. Never use raw counts.
- Two-finger pinch/rotate/page-swipe competition must have one winner. Ordinary
  M12 scrolling wins when translation commits first; pinch/rotate/page swipe
  wins only when its own normalized threshold crosses first.
- Three/four-finger swipes expose continuous translation progress.
- Edge gestures require contacts to begin within the configured edge zone.
- Thumb+three pinch/spread uses M13 thumb metadata when available; otherwise it
  is explicitly unavailable, not guessed.
- Smart Zoom is an explicit configurable semantic action; it is not assumed to
  exist in every application.
- Add `M14Profile` inheriting M13; CLI accepts `m14-gestures-v1` explicitly.

## Tests / exit

Cover every gesture begin/update/end/cancel path, threshold equality,
hysteresis, reversal, contact replacement/count changes, competition with
scroll, missing thumb metadata fallback, replay determinism, backend
unsupported behavior, CLI routing and all workspace gates.
