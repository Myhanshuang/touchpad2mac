# M15 Task — Three-Finger Drag and KDE Desktop Actions

Authority: `PHASE2_PLAN.md` M15. Implementation/review is offline/fake-backed.

## Goal

Add three-finger drag/drag-lock policy and a KDE-specific action adapter while
keeping all KDE naming/API details out of `touchpad-core`.

## Contract

- Add a three-finger drag state machine with candidate, dragging, locked,
  release and cancel behavior. It must synthesize at most one logical left
  down/up pair and route centroid movement through the existing M11 pointer
  fidelity/remainder path.
- Three-finger drag, three-finger tap and three/four-finger swipe share one
  explicit priority/ownership policy; no simultaneous owners.
- Add/extend semantic `DesktopAction` values for Overview, Present Windows,
  workspace next/previous, Show Desktop, application launcher, Notification
  Center, page next/previous, and Smart Zoom.
- Add `touchpad-desktop` KDE action mapping as configurable/discoverable data.
  Core must not contain KDE D-Bus names, shortcuts or object paths.
- Each mapping can be enabled/disabled/remapped. Unsupported transport returns
  `Unavailable`; tests use an injected fake action transport.
- Add `M15Profile` inheriting M14; CLI accepts `m15-kde-v1` explicitly.

## Tests / exit

Cover drag ownership/lock/cancel/replacement, competition with swipes,
pointer-fidelity reuse, action-map discovery/config/disable, fake KDE
transport ordering/failure cleanup, core KDE-independence, CLI routing and all
workspace gates. No real KDE action is invoked during implementation/review.
