# M16 Review — Productionization and Extension Boundaries

Decision: **APPROVED — M16 code-complete / live-unqualified**.

## Implemented

- Strict serde runtime configuration v2 with `deny_unknown_fields` semantic
  validation and explicit v1 → v2 migration. Unknown/future versions fail
  closed; persistent/autostart configuration is rejected in M16.
- Independent device/output reconnect policies and deterministic controllers:
  bounded exponential backoff, maximum retry count, reset-on-success, sticky
  idempotent stop/cancel.
- Platform-neutral service lifecycle:
  `Stopped/Starting/Running/Reconnecting/Degraded/Stopping/Faulted`, explicit
  legal transitions, idempotent shutdown entry/completion.
- Conservative capability matrix. Wayland portal/libei is implemented but
  unqualified; X11/uinput require separate adapters/qualification; continuous
  gestures and KDE actions are semantic-only at their current output
  boundaries; pressure/haptics are unsupported.
- `m16-production-v1` inherits M15 interaction policy without changing it.
- Pure CLI `config-check FILE` and `service-preflight FILE`; neither opens an
  input device, starts a portal, installs a service, enables autostart, or
  transitions the service out of `Stopped`.
- `docs/M16_ACCEPTANCE.md` and `docs/M16_CAPABILITIES.md` document future
  user-run qualification, rollback, reconnect and unsupported boundaries.

## Final gates

All passed on the final M16 workspace:

```text
cargo fmt --all -- --check                              PASS
cargo clippy --workspace --all-targets --locked -- -D warnings PASS
cargo test --workspace --locked                         PASS
cargo test --release --workspace --locked               PASS
```

Observed suites include 255 touchpad-core unit tests, 57 M11 fidelity public
tests, M12/M13/M14/M15 public integration suites, 119 desktop tests, 168 Linux
tests, 104 touchpadctl unit tests and 22 public CLI integration tests, plus
trace/replay/doc tests; zero failures in debug and release.

## Review notes

No new unsafe was introduced in the M12–M16 core work; `touchpad-core` remains
`#![forbid(unsafe_code)]`. No real takeover, device grab, portal/libei output,
real KDE action, systemd/autostart install, X11/uinput fallback, pressure or
haptic claim was exercised by automated acceptance.

`m16-production-v1` means **configuration/operational code completeness**, not
live production qualification or macOS equivalence. M6/M10–M16 live evidence
remains a separate user-controlled process.
