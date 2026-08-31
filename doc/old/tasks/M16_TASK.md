# M16 Task — Productionization and Extension Boundaries

Authority: `PHASE2_PLAN.md` M16. Static/offline completion does not imply
cross-device/live production qualification.

## Goal

Make the stack operationally maintainable: versioned configuration,
device/output reconnection state machines, foreground service lifecycle,
capability reporting, upgrade/rollback, and explicit unsupported adapters.

## Contract

- Add a serde versioned runtime configuration with strict validation and an
  explicit migration path from v1; unknown future versions fail closed.
- Add deterministic device/output reconnect controllers with bounded
  exponential backoff, retry caps, reset-on-success and stop/cancel semantics.
- Add a platform-neutral service lifecycle state machine
  (Stopped/Starting/Running/Reconnecting/Degraded/Stopping/Faulted) with
  explicit transitions and idempotent shutdown.
- Add CLI `config-check`/equivalent pure validation and a foreground service
  preparation path; do not silently install/start autostart or systemd units.
- Preserve bounded takeover as the acceptance/debug path. Persistent service
  enablement remains user-controlled and live-unqualified until M6/M10–M16
  acceptance evidence exists.
- Report X11/uinput as separate adapters requiring their own qualification;
  do not silently fall back. Pressure/haptic remain explicitly unsupported
  when hardware/output capability is absent.
- Add `M16Profile` inheriting M15 and a final explicit profile name
  `m16-production-v1`; the name means configuration completeness, not live
  qualification.
- Add acceptance/runbook docs for reconnect/config rollback and a capability
  matrix; no cross-machine claim without evidence.

## Tests / exit

Cover config roundtrip/migration/rejection, reconnect/backoff transitions,
service lifecycle, capability matrix, unsupported X11/uinput/pressure/haptic,
CLI validation, all inherited profile invariants and all workspace gates.
M16 code-complete may still be live-unqualified.
