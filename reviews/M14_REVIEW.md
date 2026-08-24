# M14 Review — Continuous Gestures

Decision: **APPROVED — code-complete / live-unqualified**.

Implemented: platform-neutral continuous gesture Begin/Update/End events,
pinch/rotate/page-swipe, 3/4-finger swipe, edge gesture and thumb+3 recognition,
with candidate/commit ownership competition against M12 scroll. Public
end-to-end tests cover pinch winning before scroll, ordinary two-finger
translation remaining scroll, three-finger exclusive ownership, and thumb+3
using M13 metadata instead of geometry guesses.

The M6 desktop sink explicitly rejects continuous gesture semantic events as
unavailable; no native backend equivalence is fabricated. Final workspace
gates are recorded in M16 review.
