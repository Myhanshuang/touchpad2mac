# M15 Review — Three-Finger Drag / KDE Actions

Decision: **APPROVED — code-complete / live-unqualified**.

Implemented: three-finger drag, drag lock, three-finger tap semantic action,
priority below the 1 mm drag commit and before M14 2 mm swipe commit, reuse of
the existing pointer fidelity/remainder path, expanded platform-neutral
desktop actions, and a discoverable/remappable/disableable `KdeActionMap`
with injected transport.

Focused evidence: M15 public drag tests 3/3, pure drag tests 3/3, KDE action
adapter tests 3/3, CLI integration 22/22 and clippy passed. No real KDE action
transport is enabled, so desktop action delivery remains live-unqualified.
