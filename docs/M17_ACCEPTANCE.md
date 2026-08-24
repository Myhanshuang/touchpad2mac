# M17 Feel Tuning Acceptance — Future User-Run A/B

Automated tests qualify schema/routing/math only. They do not qualify hand
feel. M17 remains **live-unqualified** until a user performs bounded A/B
acceptance after all prerequisite M6/M10 evidence is complete.

1. Start from `touchpadctl feel-default`; archive the exact JSON.
2. Change one parameter family at a time. Validate with `feel-check`.
3. Use 10-second bounded `m17-tunable-v1 --feel-config ...` trials first.
4. Compare pointer precision, fast travel, diagonal behavior, scroll start/
   stop, momentum length, reversal, pinch/swipe false commits and three-finger
   drag ownership.
5. Reject any tuning that causes accidental gesture ownership, stuck drag,
   oscillatory axis lock, excessive pointer jitter/stickiness, or difficult
   momentum cancellation.
6. Only after stable 10-second trials proceed through the same 60/300-second
   bounded acceptance discipline. Record config hash/file, device, desktop,
   date and observations.

M17 acceptance does not retroactively qualify unsupported continuous backend
events, real KDE transport, X11/uinput, pressure or haptics.
