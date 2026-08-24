# M6 Execution Task — KDE Wayland Output Backend Qualification

You are the implementation agent. Implement **M6 only** from `PHASE2_PLAN.md`.

Before editing, read in full:

- `design.md`
- `IMPLEMENTATION_BRIEF.md`
- `DESIGN_V2.md`
- `MILESTONES.md`
- `PHASE2_PLAN.md`
- `reviews/M5_REVIEW.md`
- the current workspace source and tests

## Hard scope

Implement a safe, testable KDE Wayland output qualification slice using the official XDG RemoteDesktop portal and libei/liboeffis stack available on this host. Make a documented dependency/ABI choice based on the installed environment; do not blindly follow an example implementation. Keep desktop-specific code outside `touchpad-core`.

Required outcomes:

1. A real Linux desktop output adapter capable of translating the existing typed `OutputEvent` contract for relative pointer motion, primary/secondary buttons, and pixel-precise smooth scroll lifecycle when the negotiated device exposes those capabilities.
2. Explicit lifecycle and failure states. Portal refusal/cancel, missing session bus/library/protocol/capability, transport disconnect, partial send failure, and shutdown must be honest structured results.
3. Track emitted button/key/scroll state and provide idempotent `release_all`; normal shutdown, fatal shutdown, partial failure, and fallback Drop must not leave a logically held state. Preserve the primary failure and cleanup diagnostics.
4. `touchpadctl output-probe` whose default is non-emitting dry-run. Real desktop emission requires a separate explicit `--emit`, a visible warning/countdown, and a short fixed bounded test pattern. Tests must never emit real desktop input.
5. Fake transport/session seams and deterministic tests covering ordering, capability negotiation, backpressure/partial failure, disconnect, repeated shutdown and release behavior without a Wayland desktop.
6. Environment probing and a written manual qualification procedure for the current KDE Wayland session. Do not mark the backend qualified or suitable for takeover until a reviewer actually runs and measures `--emit`.
7. Update `README.md`, `THIRD_PARTY.md`, `DESIGN_V2.md`, and milestone documentation as necessary, while clearly separating automatic tests, read-only environment detection, and unperformed interactive validation.

## Safety and architecture constraints

- Do **not** open, read, record, or grab any physical `/dev/input` device in M6.
- Do **not** add takeover, pointer/scroll policy algorithms, tap, drag, gesture recognition, daemon/service behavior, autostart, or system-setting changes.
- Do **not** automatically move the pointer, click, or scroll during tests or ordinary probe execution.
- Do **not** create a virtual touchpad or expose raw contacts/finger count to the compositor.
- Output preparation and authorization must be designed to complete before future `EVIOCGRAB`.
- Do not claim relative motion avoids compositor acceleration or scroll reinterpretation until the manual A/B measurements demonstrate it. Represent this as an explicit unqualified state.
- Core remains free of Linux, Wayland, KDE, portal and D-Bus dependencies.
- Any new unsafe code belongs only in a minimal platform FFI boundary with nearby safety invariants; prefer a safe maintained binding when it satisfies the protocol and lifecycle requirements.
- CI and offline replay remain operable without portal, display server, session bus, system library, hardware or root.
- Never include credentials in files, logs, tests, documentation or command output.
- Do not commit or push.

## Required final checks

Run and report:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Also run only non-emitting CLI smoke/negative checks. Leave real `output-probe --emit` for the reviewer.

End with the six-part review handoff required by `PHASE2_PLAN.md`. Stop after M6; do not begin M7.
