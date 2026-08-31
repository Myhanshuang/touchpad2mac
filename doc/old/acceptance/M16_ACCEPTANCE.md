# M16 Productionization Acceptance — Future User-Run Procedure

M16 is code-complete only after offline gates/review. This document describes
future live/operational qualification and is **not executed by automated
tests**.

## 1. Preconditions

- M6 output calibration evidence is recorded.
- M10 bounded takeover 10 s → 60 s → 300 s acceptance is complete.
- M11–M15 feature-specific acceptance has been completed for every feature
  the operator intends to enable.
- `touchpadctl config-check CONFIG.json` succeeds.
- `touchpadctl service-preflight CONFIG.json` reports `state=Stopped` and the
  expected capability matrix. Preflight itself starts no service.

## 2. Configuration migration / rollback

1. Keep an immutable copy of the last known-good configuration.
2. Validate a v1 configuration and record that it migrates explicitly to v2.
3. Validate the v2 result and confirm `foreground_only=true`.
4. Confirm unknown fields and future versions are rejected.
5. Configure a distinct `rollback_profile` and verify rollback is an explicit
   operator action; no automatic silent downgrade is permitted.

## 3. Reconnect acceptance

For device and output session separately:

1. Start only in an explicitly user-controlled foreground session.
2. Induce one recoverable disconnect.
3. Record every retry delay and verify bounded exponential backoff and cap.
4. Verify success resets attempt count/backoff.
5. Repeat until retry exhaustion and verify the service enters an explicit
   degraded/faulted state instead of retrying forever.
6. Verify SIGINT/SIGTERM or explicit stop cancels future retries and performs
   ordered release exactly once.

## 4. Capability boundaries

- Wayland portal/libei remains live-unqualified until its own evidence exists.
- X11 and uinput are separate adapters requiring separate implementation and
  qualification. No silent fallback is accepted.
- Continuous gesture semantic events must not be claimed as native output on
  a backend that rejects them.
- KDE actions require an explicitly configured real transport; the injected
  test transport is not live evidence.
- Pressure and haptics remain unsupported unless both real hardware and a
  qualified output/input interface provide them.

## 5. Production qualification result

Record device model, desktop/session, distribution/kernel, configuration
version/hash, exact profile, acceptance date, failures/reconnect evidence, and
rollback result. Until that record exists, M16 remains **live-unqualified**.
