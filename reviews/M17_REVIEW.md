# M17 Review — Tunable Feel Parameters (CLI / Offline GUI)

Decision: **APPROVED — M17 code-complete / live-unqualified**.

## Implemented

- Strict standalone `FeelConfig` v1 shared by core, CLI and the generated
  offline HTML editor. It is a tuning overlay, not a replacement for M16
  runtime/safety configuration.
- Sixteen explicit user-facing controls across pointer, scroll, continuous
  gesture and three-finger-drag feel. Every numeric value has a documented
  safe editing range plus cross-field invariants.
- Ownership-sensitive validation keeps
  `drag.commit_threshold_mm < gesture.multi_swipe_commit_mm`, preserving the
  M15-before-M14 three-finger ownership priority.
- `M17Profile` inherits M16 and replaces only the four feel-related typed
  configs. Tap, two-finger base policy, contact robustness, device/output,
  reconnect, cleanup and service safety policy remain unchanged.
- The default `FeelConfig` is tested to construct an Arbiter configuration
  exactly equal to `m16-production-v1`; selecting M17 without editing the
  file therefore changes no interaction behavior.
- CLI operations:
  `feel-default`, `feel-check`, `feel-show`, `feel-set`, `feel-gui`.
- `feel-gui` generates one self-contained HTML file with embedded sliders,
  numeric inputs, validation, JSON preview and export. It has no external
  assets, network requests, server, device access or live-apply path.
- `m17-tunable-v1` is explicit and never inferred. Its bounded takeover path
  requires `--feel-config FILE`; the flag is rejected for M10–M16. The file
  is loaded and strictly validated before output factory/device/recorder/grab
  side effects.
- `docs/M17_TUNING.md` documents every exposed parameter/range/effect and the
  deliberately non-tunable safety boundaries. `docs/M17_ACCEPTANCE.md`
  defines future user-run bounded A/B acceptance.

## Final gates

After one help-text-only test correction, the complete gate sequence was
re-run from the beginning and passed with exit code 0:

```text
cargo fmt --all -- --check                              PASS
cargo clippy --workspace --all-targets --locked -- -D warnings PASS
cargo test --workspace --locked                         PASS
cargo test --release --workspace --locked               PASS
```

Observed suites on the final tree include 260 touchpad-core unit tests,
57 public M11 fidelity tests, M12/M13/M14/M15 public suites, 119 desktop
tests, 168 Linux unit tests plus Linux integration suites, 74 trace unit
tests, 108 touchpadctl unit tests and 22 public CLI integration tests, plus
release and doc-test counterparts; zero failures.

## Review boundaries

- No new unsafe was introduced in `touchpad-core`; it remains
  `#![forbid(unsafe_code)]`.
- M17 does not expose takeover confirmation, grab, duration, output
  qualification, cleanup, reconnect, service lifecycle, device quirks, tap
  timing, X11/uinput fallback, pressure or haptic policy as feel controls.
- The generated GUI is an editor only. It does not hot-apply settings to a
  live session.
- No real takeover, physical grab, portal/libei output, KDE action transport,
  service install/autostart, X11/uinput, pressure or haptic operation was run
  during automated qualification.

M17 therefore remains **live-unqualified** until the separate user-run A/B
procedure in `docs/M17_ACCEPTANCE.md` is completed. No macOS-equivalence claim
is made.
