# M17 Task — Tunable Feel Parameters (CLI / Offline GUI)

Authority: follows approved M16 productionization. M17 must not weaken M10
safety, cleanup, explicit profile selection, or live qualification boundaries.

## Goal

Expose a deliberately small set of parameters with a strong perceptual effect
on pointer, scroll, continuous-gesture and three-finger-drag feel. CLI and GUI
must use the same strict versioned `FeelConfig` schema.

## Contract

- Add `FeelConfig` v1 as a strict standalone tuning overlay. It is not a
  replacement for M16 runtime/safety configuration.
- Tunable families:
  - pointer: dead-zone radius, tracking speed, low/high gain;
  - scroll: low/high gain, axis-lock engage/release ratios, momentum decay and
    start/stop speeds;
  - gestures: pinch/page/multi-finger commit thresholds;
  - three-finger drag: drag commit threshold and drag-lock enable.
- Every parameter has an explicit safe editing range and cross-field
  validation. The default tuning document must reproduce M16/M15 behavior
  exactly.
- Add `m17-tunable-v1`, opt-in only. It inherits M16 and replaces only the
  four feel-related configs; no tap, device, output, grab, reconnect or
  cleanup policy is changed.
- CLI must provide strict check/show/set/default operations without touching
  hardware. M17 takeover may use an explicit `--feel-config FILE` only with
  `--profile m17-tunable-v1`; the file is validated before any live side
  effect. Existing profiles reject `--feel-config`.
- GUI is a generated self-contained offline HTML editor. No server, network,
  browser automation or live device application is performed. It edits and
  exports the exact same FeelConfig JSON accepted by CLI.
- Document every exposed parameter, units, range and likely perceptual effect.

## Exit

Public tests cover defaults == M16, range/cross-field validation, CLI editing,
HTML generation, profile routing and no-safety-policy change. Final fmt,
workspace clippy, debug tests and release tests must pass before review.
M17 remains live-unqualified until user-run A/B acceptance.
