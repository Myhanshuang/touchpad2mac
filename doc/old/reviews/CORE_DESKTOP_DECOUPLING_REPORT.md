# Core / Desktop Environment Decoupling Review

Date: 2026-08-24

Scope: current `/home/acacia/touchpad` workspace, with emphasis on the boundary
between `touchpad-core`, Linux input/runtime code, desktop output adapters, and
the `touchpadctl` composition layer. This review is architectural only; it does
not change runtime behavior.

## Executive summary

Overall assessment: **B+ / roughly 7.5–8 out of 10**.

The important distinction is that the **gesture engine itself is already well
decoupled**, while some **production/configuration metadata and runtime
composition decisions have leaked back across the intended boundary**.

The current project is not in a state where KDE/Wayland implementation details
are mixed into pointer/scroll/tap/drag algorithms. The main interaction path is
actually close to a ports-and-adapters architecture:

```text
Linux evdev / decoder
        |
        v
normalized ContactFrame
        |
        v
touchpad-core Arbiter
        |
        v
semantic OutputEvent / OutputSink
        |
        v
desktop adapter
  |- portal + libei pointer/button/scroll
  `- KDE KGlobalAccel semantic actions
```

The two most important architectural defects are elsewhere:

1. `touchpad-core::production` contains concrete deployment/backend facts such
   as `WaylandPortalLibei`, `X11Adapter`, `UinputAdapter`, and `KdeActions`.
   This contradicts the crate's own platform-neutral contract and means that
   adding/changing a desktop backend requires modifying core metadata.
2. `touchpadctl takeover` currently treats **`m19-live-v1` as an implicit KDE
   backend selector**. The M19 core policy itself is desktop-neutral, but the
   application layer checks the profile name and chooses
   `RealKdeStreamingOutputFactory`. This creates a runtime coupling between a
   core policy profile and one desktop environment.

There is also a medium-term extensibility issue inside `touchpad-desktop`:
`StreamingOutput` is named as a generic desktop-output session contract, but
its public lifecycle/error/capability types are still heavily shaped around
the portal/EIS/libei implementation. That is acceptable for today's single
real backend, but it will become friction when a second substantially
different backend is added.

The recommended response is **not** to split every crate now. Preserve the
existing gesture engine and `OutputSink` boundary. First remove the two wrong
dependency directions above; only then generalize the desktop session facade
when a second backend actually needs it.

## 1. Current dependency graph

`cargo tree` confirms the compile-time direction is clean:

```text
touchpad-core
  |- serde
  `- thiserror

touchpad-desktop
  `- touchpad-core

touchpad-linux
  |- touchpad-core
  `- touchpad-trace

touchpadctl
  |- touchpad-core
  |- touchpad-linux
  |- touchpad-desktop
  `- touchpad-trace
```

`touchpad-core` has no compile dependency on Linux, Wayland, zbus, libei,
KDE, X11, or the desktop crate. This is a strong foundation and should be
preserved.

The dependency DAG also has the correct broad shape: the application is the
composition root and depends on both the input and output sides; the Linux and
desktop crates do not depend on each other.

### Score: compile-time dependency purity — 9.5/10

The missing 0.5 is not a Cargo issue; it reflects semantic/backend metadata
that still lives in core source despite the clean package graph.

## 2. Core interaction engine boundary

The strongest part of the current architecture is the runtime interaction
pipeline.

### 2.1 Normalized input boundary is correct

`touchpad-core::DeviceDescriptor`, `Contact`, `ContactFrame`, typed units, and
`Monotonic` isolate gesture policy from Linux raw input details. Linux-specific
Type-B slots, ioctl state and evdev parsing remain in `touchpad-linux` and are
translated into the normalized core model.

This is the correct direction. A future Windows/macOS input adapter should be
able to produce the same core contact frames without changing gesture code.

### 2.2 Output boundary is also correctly semantic

`touchpad-core::OutputEvent` contains resolved semantic operations:

- relative logical pointer movement;
- button down/up;
- scroll lifecycle/deltas;
- key edges;
- semantic desktop actions;
- continuous gesture events.

`OutputSink` is synchronous and explicitly defines ordering, accepted-prefix
failure behavior, frame submission and idempotent release. The core does not
call D-Bus, Wayland, libei or KWin directly.

This is the right abstraction level for the gesture engine. In particular,
`DesktopAction::{NextWorkspace, OpenOverview, ShowDesktop, ...}` should **not**
be considered a KDE leak. These are product-level semantic intents. The KDE
adapter decides how an intent maps onto KGlobalAccel; another desktop adapter
can map the same intent differently.

The closed `DesktopAction` enum does mean that introducing a brand-new
user-facing semantic action requires a core API change. That is normal domain
evolution, not desktop-environment coupling.

### 2.3 `TakeoverBridge<S: OutputSink>` is an especially good seam

`touchpad-linux::TakeoverBridge` is generic over `S: OutputSink`. It only knows
about `Arbiter`, `ContactFrame`, `OutputSink`, accepted-prefix/fail-stop
semantics and cleanup. Its documentation mentions the portal/libei production
instance, but its type graph does not depend on the desktop crate.

That lets tests use recording/fault-injecting sinks and lets production inject
the desktop output session without teaching the Linux input runtime about KDE
or Wayland.

This seam should remain untouched by the architectural cleanup.

### Score: algorithm / input-output boundary — 9/10

## 3. Desktop adapter isolation

The concrete desktop implementation is mostly where it belongs.

### 3.1 KDE identifiers stay in `touchpad-desktop`

`crates/touchpad-desktop/src/kde_actions.rs` owns:

- `org.kde.kglobalaccel`;
- `org.kde.KWin`;
- KWin object paths/interfaces;
- KGlobalAccel shortcut action IDs;
- KDE-specific capability probing and invocation.

The mapping boundary consumes `touchpad_core::DesktopAction` and turns it into
KDE-specific calls. This is exactly the desired dependency direction.

### 3.2 Portal and libei are behind injected seams

`Portal` and `Transport` are injected traits. `PortalOutputSink` contains the
portal/EIS lifecycle and implements core's `OutputSink`. Tests can use
`FakePortal` and `FakeTransport` without opening a real session or emitting
desktop input.

The native libei FFI stays crate-private. This gives both safety containment
and a clean semantic boundary.

### 3.3 Desktop action output is composed rather than pushed into core

`KdeActionStreamingOutput<T>` wraps an inner streaming output:

```text
Pointer / button / scroll -> portal + libei
DesktopAction             -> KDE action adapter
```

This is a good composition. The arbiter does not need to know that two wire
channels are involved.

### Score: concrete desktop implementation isolation — 8.5/10

## 4. High-priority leak: backend/deployment metadata inside core

This is the clearest violation of the stated architecture.

`crates/touchpad-core/src/lib.rs` says that the crate MUST NOT depend on Linux,
Wayland, X11, KDE or GNOME. The compiled dependency graph obeys that statement,
but `crates/touchpad-core/src/production.rs` contains concrete backend and
qualification concepts:

```text
OutputAdapter::WaylandPortalLibei

CapabilityId::WaylandPortalLibei
CapabilityId::X11Adapter
CapabilityId::UinputAdapter
CapabilityId::KdeActions
```

The capability matrix also embeds operational statements such as the current
KDE Plasma KGlobalAccel support and whether X11/uinput have been qualified.

These facts are not gesture-engine facts. They belong to the application /
runtime composition layer or to the concrete adapter registry.

### Why this matters

If a GNOME action adapter, X11 backend or another output transport is added,
the current model requires editing `touchpad-core` merely to describe the
adapter. That turns the supposedly stable inner layer into a registry of outer
infrastructure.

It also creates update drift. That drift is already visible:

- `touchpad-core::production::validate_profile_name` only accepts M10 through
  M16 profile names;
- the real takeover path supports M17, M18 and M19;
- `OutputAdapter` is not used by the real takeover backend-selection path;
  outside core it is effectively only consumed by the M16 config/preflight
  reporting path.

So there are currently **two partially independent runtime control planes**:

1. M16 `RuntimeConfig` / `OutputAdapter` / capability-matrix reporting;
2. the actual M19 takeover composition code selecting real factories.

That is a stronger warning than a naming issue: the misplaced layer has
already started becoming stale relative to the active runtime.

### Recommendation

Move the application/runtime concerns out of `touchpad-core`.

Best long-term shape:

```text
touchpad-core
    gesture domain + semantic contracts only

touchpad-linux
    Linux input adapter/runtime

touchpad-desktop
    desktop output adapters

touchpad-runtime        <- new application/service composition crate
    RuntimeConfig
    profile registry/selection
    backend registry/selection
    reconnect controllers
    service lifecycle
    capability/qualification reporting
    composition of linux + core + desktop

touchpadctl
    CLI only; calls touchpad-runtime
```

If introducing a new crate is considered premature, the same code can first
move under `apps/touchpadctl/src/runtime/`. A dedicated runtime crate becomes
worthwhile once a second frontend/service needs the same composition logic.

`ReconnectPolicy`, `ReconnectController`, and generic lifecycle state machines
are themselves platform-neutral, but they are still application/runtime
concepts rather than touch gesture concepts. Moving `production.rs` as a unit
first gives the cleanest boundary; reusable generic pieces can be extracted
later if another runtime needs them.

### Severity: HIGH

## 5. High-priority implicit coupling: M19 selects KDE

The most important runtime coupling is in
`apps/touchpadctl/src/cmd/takeover.rs`.

Current behavior is effectively:

```text
if real production output && profile == m19-live-v1:
    validate settings with required_real_kde_actions(...)
    use RealKdeStreamingOutputFactory
else:
    use RealStreamingOutputFactory
```

The same KDE-specific validation is repeated for hot reload when
`real_kde_live` is true.

The core `M19Profile` itself contains no KDE calls and constructs a neutral
`ArbiterConfig`, so compile-time layering still looks correct. But at runtime,
the profile identity has become an implicit desktop-environment selector.

### Consequence

Trying to run the exact same M19 interaction policy on GNOME would not simply
swap the action adapter. The current composition path would attempt KDE
KGlobalAccel preflight because the profile is M19.

That couples two independent choices:

- **interaction policy**: M10/M11/.../M19;
- **desktop output implementation**: portal/libei + KDE actions, future GNOME
  actions, X11, test/fake, etc.

Those axes must be independently selectable.

### Recommendation

Make the application composition explicit:

```text
SelectedPolicy
    profile_name
    ArbiterConfig
    semantic action requirements

SelectedDesktopBackend
    pointer/scroll backend
    desktop-action provider
    negotiated capabilities
```

Conceptually:

```text
profile = m19-live-v1
pointer_backend = portal-libei
desktop_actions = kde | gnome | none | fake
```

The CLI/runtime composition root is allowed to know desktop environments; that
is its job. The mistake is deriving the desktop environment from the core
profile name.

For capability checking, prefer an adapter-facing semantic interface such as:

```text
ActionSupport::supports(DesktopAction)
ActionSupport::validate_required(&[DesktopAction])
```

instead of making higher layers call a KDE-named validator whenever a certain
profile is selected.

### Severity: HIGH

## 6. Medium-priority issue: the generic streaming facade is portal-shaped

`touchpad-desktop::StreamingOutput` is intended as the reusable output-session
contract, but its interface exposes types shaped by the current portal/libei
backend:

- `OutputCapabilities` is defined from libei pointer/button/scroll bits;
- `SessionState` contains `Authorizing`, `Ready`, `Emulating`, `Interrupted`;
- `DesktopOutputError` is dominated by D-Bus portal/EIS/libei variants such as
  `NoSessionBus`, `PortalUnavailable`, `AuthorizationRefused`,
  `LibraryMissing`, `DevicePaused`, and `InvalidPortalPath`.

For the current real backend this is internally coherent. The issue is the
name and role of `StreamingOutput`: a future output implementation with no
portal authorization or EIS device lifecycle would have to pretend to inhabit
these states or force a redesign.

### Recommendation

Do not rewrite this before another backend exists. When the second backend is
introduced, place one neutral session contract outside the portal-specific
implementation, for example:

```text
trait OutputSession: OutputSink {
    prepare(...) -> SemanticOutputCapabilities
    health() -> OutputHealth
    release_all(...)
}

OutputHealth:
    New | Preparing | Ready | Interrupted | Stopping | Stopped | Faulted
```

Portal/EIS-specific state can remain observable through the concrete adapter
for diagnostics without becoming mandatory for every backend.

Similarly, capability reporting should be expressed in semantic output terms
at the generic boundary, then mapped from libei bits inside the libei adapter.

### Severity: MEDIUM

## 7. Medium/low issue: `m15-kde-v1` lives in core

`touchpad-core::m15` is named and serialized as `m15-kde-v1`, while the code
inside that module only builds a three-finger-drag policy on top of M14.

This is mostly historical naming, but names are architecture: it suggests
that the core profile itself is KDE-specific even though the implementation
is not.

Do not break existing persisted profile strings solely for cosmetic cleanup.
Instead:

1. keep `m15-kde-v1` as a compatibility/migration alias;
2. use neutral names for future profiles;
3. when the runtime config schema next changes, migrate the historical name to
   a neutral policy identifier if compatibility policy allows it.

### Severity: MEDIUM-LOW

## 8. Low-priority textual leakage inside core

Searches find platform terms in core comments/docs, for example:

- `OutputSink::submit_frame` says a protocol backend "notably libei" may use
  a hardware frame boundary;
- some arbiter test comments compare behavior to uinput;
- M10/M11 comments say values are not read from KDE/libinput;
- the core crate invariant itself lists Wayland/X11/KDE/GNOME.

These references do not create runtime coupling. They are documentation or
test references and are safe to leave while higher-priority issues are fixed.

After the larger cleanup, generic comments can be rewritten to describe the
contract without naming one current adapter, but this is not an architectural
blocker.

### Severity: LOW

## 9. What should stay in core

The following current responsibilities are correctly placed and should not be
moved into desktop-specific code:

- normalized contacts and physical units;
- pointer/scroll/tap/tap-drag/three-finger-drag algorithms;
- gesture recognition and ownership arbitration;
- fidelity/robustness processing;
- gesture-to-**semantic-action** mapping;
- `DesktopAction` semantic intents;
- `ContinuousGestureEvent` semantics;
- `OutputEvent` and `OutputSink`;
- user feel/gesture policy settings, provided they remain backend-neutral.

Moving gesture recognition or raw contacts into the KDE adapter would make the
architecture worse. The current design correctly resolves gestures before the
desktop boundary.

## 10. What should not remain in core

The following should migrate outward:

- concrete output adapter names (`WaylandPortalLibei`, X11, uinput);
- KDE capability/qualification state;
- environment/backend availability matrices;
- runtime backend selection;
- physical device path selection;
- service/preflight reporting that describes installed adapters;
- profile registry logic whose purpose is application configuration rather
  than gesture policy construction.

## 11. Recommended target architecture

```text
                    +--------------------+
                    |   touchpad-core    |
                    |--------------------|
                    | ContactFrame       |
                    | Arbiter            |
                    | UserSettings       |
                    | OutputEvent        |
                    | OutputSink         |
                    +---------^----------+
                              |
                +-------------+-------------+
                |                           |
       +--------+---------+       +---------+----------+
       | touchpad-linux   |       | touchpad-desktop  |
       |------------------|       |-------------------|
       | evdev            |       | portal/libei      |
       | decoder          |       | KDE actions       |
       | grab/runtime     |       | future GNOME/X11  |
       | generic bridge   |       | adapters          |
       +--------^---------+       +---------^---------+
                |                           |
                +-------------+-------------+
                              |
                    +---------+----------+
                    | touchpad-runtime   |
                    |--------------------|
                    | profile selection  |
                    | backend selection  |
                    | capabilities       |
                    | reconnect/service  |
                    | composition        |
                    +---------^----------+
                              |
                    +---------+----------+
                    |   touchpadctl      |
                    +--------------------+
```

The key rule is:

> **core decides what an interaction means; desktop adapters decide how that
> meaning is delivered; runtime decides which adapters are present and how
> they are composed.**

## 12. Migration plan

### P1 — sever profile -> KDE backend coupling

Do this before adding more desktop-specific behavior.

1. Introduce an explicit desktop/backend selection concept in the composition
   layer.
2. Stop checking `profile_name == m19-live-v1` to select KDE output.
3. Resolve semantic action requirements independently from the selected
   backend.
4. Make hot-reload validation use the active backend's capability validator,
   not `required_real_kde_actions` selected through the profile name.
5. Add a regression proving that an M19 profile can be composed with a fake
   non-KDE action provider without touching KDE preflight.

This is the highest-value change because it immediately makes "same core on
another desktop environment" structurally possible.

### P1 — move runtime/backend registry out of core

1. Move `RuntimeConfig`, `OutputAdapter`, capability-matrix/reporting and
   service composition policy from `touchpad-core::production` into an
   application/runtime module or new crate.
2. Preserve the existing v1/v2 serialized configuration migration if users
   may already possess those files.
3. Make the actual takeover composition consume the same runtime model, or
   remove fields that remain preflight-only. Avoid maintaining a second stale
   backend registry.
4. Keep core re-exports only temporarily as deprecated compatibility aliases
   if external API compatibility matters.

### P2 — genericize the output-session facade when adding backend #2

1. Define generic semantic capabilities and generic session health.
2. Keep portal/EIS detailed states/errors inside the portal implementation.
3. Adapt the existing portal streaming session to the neutral facade.
4. Add the second real backend without changing core.

### P2 — neutralize historical profile naming

Use neutral names for future profiles. Migrate `m15-kde-v1` only through an
explicit config-version migration, not a silent rename.

### P3 — architecture guardrails

Add CI checks that make regression harder:

1. assert `touchpad-core` direct dependencies remain only the approved
   platform-neutral set;
2. assert `touchpad-linux` never depends on `touchpad-desktop` and vice versa;
3. add an allowlisted source scan for concrete backend identifiers under
   `touchpad-core/src` so new `org.kde.*`, `libei`, `WaylandPortalLibei`, etc.
   cannot silently re-enter production core code;
4. test one identical sequence of normalized `ContactFrame`s against several
   fake output adapters and assert identical core `OutputEvent` decisions;
5. test that backend selection is orthogonal to profile selection.

## 13. Suggested acceptance criteria for the refactor

The decoupling work can be considered complete when all of these are true:

1. `cargo tree -p touchpad-core` still contains no platform/backend crates.
2. No concrete backend enum or KDE/X11/uinput qualification matrix lives in
   `touchpad-core`.
3. `m19-live-v1` can be constructed and run against a fake non-KDE output
   session without special test-only bypasses.
4. The application chooses M19 and KDE as two separate decisions.
5. Hot reload asks the selected backend whether new semantic actions are
   supported; it does not branch on `real_kde_live` derived from the profile.
6. `touchpad-linux::TakeoverBridge` remains generic over `OutputSink` and does
   not gain a desktop dependency.
7. KDE/KWin bus names and action IDs remain confined to the KDE adapter.
8. A future GNOME action adapter can be added without editing pointer/tap/
   scroll/drag/gesture algorithms in core.

## Final verdict

The **core algorithm architecture is healthy** and worth preserving. The main
technical debt accumulated after M16 is not that KDE code invaded the gesture
engine; it is that application/runtime concerns were partially placed in core
and that M19 became an implicit signal for selecting the KDE implementation.

Therefore this is **not a rewrite problem**. A relatively narrow layering
cleanup can get the project from "good single-environment architecture" to a
genuinely desktop-pluggable architecture:

1. separate policy selection from backend selection;
2. move concrete backend/runtime registry data out of core;
3. only then generalize the streaming-session facade when another backend
   arrives.

Until those changes are made, the current code is safe to continue tuning for
KDE/Wayland, but new desktop-environment support should not be layered on top
of the existing `profile == M19 -> KDE` convention.
