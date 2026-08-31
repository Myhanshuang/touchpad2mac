# Light Core / Desktop Decoupling Refactor Review

Date: 2026-08-24

## Git safety boundary

The workspace previously contained an empty `.git` directory rather than a
usable repository. A real repository was initialized before refactoring.

The complete pre-refactor workspace state was committed as:

```text
6d4838d snapshot: stable-01 pre-decoupling baseline
```

Both `stable-01` and `main` were created from that exact commit. All changes
described below were then made only on `main`; `stable-01` remains the rollback
point for the pre-decoupling implementation, including the R12 three-finger
drag work, settings, reviews, and recorded tuning traces.

## Scope

This is deliberately a **light** decoupling pass. It does not:

- change pointer/scroll/tap/tap-drag/three-finger-drag algorithms;
- change the R12 three-finger entry behavior;
- change M19 fidelity constants or live settings;
- introduce a new `touchpad-runtime` crate;
- migrate the M16 runtime-config schema;
- rename historical profile identifiers such as `m15-kde-v1`;
- generalize the portal/EIS `StreamingOutput` lifecycle yet.

The pass addresses the highest-value coupling identified by
`reviews/CORE_DESKTOP_DECOUPLING_REPORT.md` without forcing a compatibility
migration.

## 1. Core profile no longer selects KDE

Before this refactor, the real takeover path effectively contained:

```text
profile == m19-live-v1
    -> validate with required_real_kde_actions(...)
    -> RealKdeStreamingOutputFactory
```

That made a core interaction profile double as a desktop-environment selector.

The new composition boundary is explicit:

```text
main / application environment
    -> RealDesktopBackend::{PortalLibei, KdeComposite}

core profile
    -> ArbiterConfig
```

The two choices are independent.

`main.rs` now inspects the desktop environment once at the composition root:

- `XDG_CURRENT_DESKTOP` containing `kde`, or
- `KDE_FULL_SESSION` being present

selects `KdeComposite`; otherwise the real backend is `PortalLibei`.

No comparison with `m19-live-v1` participates in that choice.

Tests that inject their own `StreamingOutput` explicitly use the inert
`PortalLibei` selection, which is ignored whenever the injected factory is
present.

## 2. Backend composition moved behind `RealDesktopPlan`

New module:

```text
apps/touchpadctl/src/desktop_backend.rs
```

It owns the application-side real backend plan:

```text
RealDesktopPlan::PortalLibei
RealDesktopPlan::KdeComposite { required_actions }
```

Responsibilities now isolated there:

- convert `RealDesktopBackend` into a prepared backend plan;
- validate loaded semantic gesture actions for the KDE backend;
- instantiate `RealStreamingOutputFactory` or
  `RealKdeStreamingOutputFactory`;
- validate hot-reloaded settings against the active real backend.

`takeover.rs` no longer imports `required_real_kde_actions`,
`RealKdeStreamingOutputFactory`, or `RealStreamingOutputFactory` directly. It
only asks the selected `RealDesktopPlan` to create output and validate reloads.

This keeps KDE-specific composition in the application layer while preserving
the existing desktop adapter implementation.

## 3. Hot reload validates through the active backend plan

The old run loop carried a `real_kde_live: bool` and branched directly into
`required_real_kde_actions`.

It now receives:

```text
Option<&RealDesktopPlan>
```

and delegates reload validation to:

```text
plan.validate_reload(settings)
```

This removes the profile-derived KDE flag from the event loop and gives a
single place to extend validation when another desktop action provider is
introduced.

## 4. Backend qualification matrix removed from `touchpad-core`

The following application/deployment metadata was removed from
`touchpad-core::production`:

```text
CapabilityId
CapabilityStatus
CapabilityEntry
capability_matrix()
```

That matrix described concrete outer-world facts such as:

- Wayland portal + libei qualification;
- future X11/uinput adapters;
- KDE KGlobalAccel support;
- pressure/haptics availability.

Those are not gesture-domain facts. The matrix now lives privately in:

```text
apps/touchpadctl/src/cmd/config.rs
```

where `service-preflight` actually consumes it.

This also removes the public re-exports of that concrete capability registry
from `touchpad-core::lib`.

## 5. Deliberately retained compatibility debt

`touchpad-core::production::RuntimeConfig` still contains:

```text
OutputAdapter::WaylandPortalLibei
```

This is a known remaining platform detail in core. It is retained in this
light pass because it is part of the existing M16 serialized config v2 and v1
migration behavior. Removing it cleanly should happen with an explicit config
schema migration rather than silently changing persisted configuration.

Important boundary after this refactor:

- this historical field remains for config compatibility;
- it no longer drives the active M19 takeover backend selection;
- real takeover composition is owned by the application-side desktop plan.

The next larger architecture pass can move the entire runtime config/service
model out of core and introduce config v3 if desired.

## 6. Files changed

```text
apps/touchpadctl/src/desktop_backend.rs       new application composition seam
apps/touchpadctl/src/env.rs                   explicit RealDesktopBackend
apps/touchpadctl/src/main.rs                  desktop detection at composition root
apps/touchpadctl/src/cmd/takeover.rs          consumes RealDesktopPlan
apps/touchpadctl/src/cmd/takeover/tests.rs    updated injected seam
apps/touchpadctl/tests/cli.rs                 updated injected seam
apps/touchpadctl/src/cmd/config.rs            owns capability qualification matrix
crates/touchpad-core/src/production.rs        drops concrete capability matrix
crates/touchpad-core/src/lib.rs               drops capability-matrix re-exports
```

No gesture algorithm source file was modified.

## 7. Regression/architecture checks

The new `desktop_backend` module includes a regression proving that backend
selection can construct `PortalLibei` and `KdeComposite` plans without any
core profile identity being involved.

Source inspection after the refactor confirms:

- `takeover.rs` no longer imports or calls `required_real_kde_actions`;
- `takeover.rs` no longer instantiates either real desktop factory directly;
- actual backend choice is carried by `TakeoverSeams::real_desktop_backend`;
- the concrete capability qualification registry no longer exists in core;
- `touchpad-linux::TakeoverBridge<S: OutputSink>` remains unchanged and
  desktop-neutral.

## 8. Full gates

All gates pass after the refactor:

```text
cargo fmt --all -- --check                                      PASS
cargo clippy --workspace --all-targets --locked -- -D warnings  PASS
cargo test --workspace --locked                                 PASS
cargo test --release --workspace --locked                       PASS
cargo build --release -p touchpadctl --locked                   PASS
```

The release binary was rebuilt successfully.

## Verdict

**APPROVED — light decoupling refactor is structurally complete.**

The project now has a clearer three-way responsibility split:

```text
touchpad-core
    decides what contacts mean

touchpad-desktop
    implements concrete desktop transports

touchpadctl application composition
    chooses which desktop transport to use
```

The stable pre-refactor implementation remains recoverable from `stable-01`.
The main remaining architectural debt is the historical M16
`RuntimeConfig::OutputAdapter` field and the portal-shaped generic streaming
session facade; neither needs to be changed as part of the current three-finger
drag debugging work.
