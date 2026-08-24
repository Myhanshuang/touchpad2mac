# M13 Task — Contact Robustness

Authority: `PHASE2_PLAN.md` M13. Implementation is offline/fake-backed.

## Goal

Add explicit contact classification/filtering for palms, thumbs, edge starts,
typing suppression, jitter, and contact replacement without inventing sensor
features a device does not report.

## Contract

- Add a pure/stateful `robustness` module with `ContactRole`, validated
  `RobustnessConfig`, feature-availability metadata, and deterministic state.
- Classifier inputs are only normalized `ContactFrame` fields plus an explicit
  external typing-activity signal. No hidden libinput/KDE state.
- Missing pressure/major/minor/orientation must select a documented fallback;
  no fabricated value and no feature-dependent rule when the feature is absent.
- Edge-start suppression is tracking-id sticky until that contact ends.
- Palm suppression may use contact major/minor only when reported and enabled.
- Thumb classification is metadata; it is retained for later gestures unless a
  configured suppression rule explicitly excludes it.
- Typing suppression uses `Arbiter::note_typing(timestamp)`/equivalent injected
  signal and a checked monotonic timeout. If no typing signal exists, the
  capability reports unavailable rather than pretending to suppress typing.
- Contact jitter filtering is bounded radial hold/release in millimeters and
  must never fabricate motion or change tracking ids.
- Replacement/discontinuity clears stale classifier state deterministically.
- Add a known `DeviceProfile` for CIRQ1080 vendor/product 0x0488/0x1054 with
  only observed/structural quirks; generic rules must not depend on it.
- Add `M13Profile` by inheriting M12 and adding robustness config; CLI accepts
  `m13-robust-v1` explicitly.

## Tests / exit

Cover every feature-present/missing fallback, edge stickiness, palm/retained
thumb behavior, typing timeout, jitter, replacement/discontinuity, CIRQ1080
profile selection, M12 compatibility, CLI routing and all workspace gates.
Live typing/palm behavior remains unqualified until user acceptance.
