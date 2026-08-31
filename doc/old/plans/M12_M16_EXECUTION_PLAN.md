# M12–M16 Execution Plan

The implementation advances strictly milestone by milestone. Each milestone
gets: contract → focused implementation → focused gates → full workspace gates
→ independent review document → repair if needed. No later milestone may be
used to hide a failing earlier gate.

## M12 — Scroll Fidelity / Momentum

1. Build pure `scroll_fidelity` config/state/math tests.
2. Add `ArbiterConfig` optional scroll fidelity + M12 profile.
3. Integrate committed two-finger deltas and lifecycle cancellation.
4. Add monotonic `Arbiter::tick`, `ArbiterSink::tick`, `TakeoverBridge::tick`.
5. Drive active momentum from the bounded loop with a finer poll quantum.
6. Add CLI profile, replay/fake-loop tests, acceptance doc, full gates/review.

## M13 — Contact Robustness

1. Build classifier/filter module and feature-availability fallbacks.
2. Integrate classifier state atomically into Arbiter; expose typing signal API.
3. Add CIRQ1080 known-device profile selection without generic dependency.
4. Add M13 profile/CLI, trace regressions, full gates/review.

## M14 — Continuous Gestures

1. Extend semantic output with platform-neutral continuous gesture events.
2. Build pure geometry recognizer and ownership competition.
3. Integrate 2/3/4-finger recognition without regressing M12 scrolling.
4. Add M14 profile/CLI, unsupported-backend behavior, replay tests.
5. Full gates/review.

## M15 — Three-Finger Drag / KDE Actions

1. Add three-finger drag + lock state machine and ownership arbitration.
2. Route drag motion through existing pointer fidelity.
3. Expand platform-neutral desktop actions.
4. Add configurable KDE action map + injected transport/fakes.
5. Add M15 profile/CLI, full gates/review.

## M16 — Productionization

1. Versioned runtime config + migrations and validation.
2. Device/output reconnect/backoff state machines.
3. Foreground service lifecycle and CLI config validation/preflight.
4. Capability matrix and explicit unsupported adapter/haptic boundaries.
5. M16 final profile, docs, all gates and final review.

## Safety / qualification boundary

All automated work is offline/fake-backed. Do not execute real takeover,
real device grabs, real portal/libei/KDE actions, system settings, daemon
installation or autostart during implementation/review. M10–M16 live
qualification remains a separate user-run process.
