# M19 Acceptance — Live Settings Hot Reload

Status: **written, not executed live**. M19 remains live-unqualified.

## 1. Offline gates

The four workspace gates from `M19_TASK.md` must pass first.

Automated evidence must cover:

- unchanged settings file -> no work;
- invalid/partially-written changed file -> reload rejected, last-good kept;
- a later valid save -> automatic recovery;
- active interaction -> valid config is queued, not applied;
- neutral boundary -> queued config applies atomically;
- multiple busy updates -> latest valid update wins;
- filter/router state resets only at the neutral apply boundary;
- all previous M10 cleanup/fault tests still pass.

## 2. User-run live sequence

Prerequisites: M6 output qualification evidence, the M10 10/60/300-second
acceptance, external keyboard/mouse, second terminal, and an already validated
M19 settings file.

Regenerate the current KDE-safe macOS-inspired preset after updating the
binary; older M18-era preset files may still contain page/notification/lookup
or native-continuous routes that real M19 now correctly rejects:

```bash
touchpadctl settings-macos settings.json
touchpadctl settings-check settings.json
```

Terminal A:

```bash
touchpadctl takeover DEVICE TRACE \
  --takeover \
  --confirm TAKEOVER \
  --output-qualified \
  --profile m19-live-v1 \
  --settings settings.json \
  --watch-settings \
  --max-duration-seconds 60
```

Terminal B, while the session is running:

```bash
touchpadctl settings-patch settings.json feel.pointer.tracking_speed=1.25
touchpadctl settings-patch settings.json feel.scroll.momentum_tau_ms=450
touchpadctl settings-patch settings.json gesture.four-finger-swipe-up=show-desktop
```

For each edit, first release all fingers/buttons. Confirm Terminal A reports an
applied generation. Then repeat the motion and record the subjective/observable
difference.

Exercise one-finger tap-and-drag explicitly: perform one qualifying tap, then
place the finger down again **within 180 ms** and move far enough to cross the
pointer threshold. This follow-up contact must begin the drag. Repeat with the
second contact starting just after 180 ms (for example about 200-250 ms after
the tap release); that motion must be an ordinary pointer interaction with no
held-left drag. A committed drag's clean lift must release the dragged object
immediately; the next pointer/tap action must not inherit held-left ownership
and no extra unlock tap is required.

Exercise three-finger drag with a desktop/application icon and deliberately
lift the fingers slightly unevenly. A committed M19 drag must remain owned for
the complete clean `3 -> 2 -> 1 -> 0` tail; the remaining contacts must not
become ordinary pointer/scroll input, and exactly one release/drop must occur
when the original contact cluster becomes empty. If the tracked reference
finger lifts before the others, the replacement reference frame must produce
no pointer jump.

During the same three-finger drag, compare the hardware cursor with the dragged
icon while moving quickly and changing direction. M19 keeps the ordinary
pointer feel unchanged but caps only the three-finger drag high-speed gain at
1.6. On each fresh gesture, the displacement used to cross the 0.8 mm commit
threshold is classification-only: the commit frame must not move the pointer
or press left. The first later reference-finger delta that emits a PointerMove
must establish `ButtonDown + PointerMove` in the same semantic/EIS frame.
Repeat at least five separate three-finger drags in alternating directions:
every drag must begin under the current pointer rather than being shifted by
the new gesture's pre-commit vector (which commonly points opposite the
previous drag when dragging the same icon back and forth).

## 3. Real KDE DesktopAction sequence

The production M19 session preflights the exact configured KDE actions before
grab. On the current Plasma 6 backend, test the supported actions one at a
time after pointer/scroll behavior is known-good:

1. four-finger swipe left/right -> next/previous workspace;
2. four-finger swipe up -> open Overview;
3. four-finger swipe down -> close Overview;
4. thumb+three spread -> Show Desktop;
5. thumb+three pinch -> Application Launcher.

For each action, record whether exactly one KDE action fires and the M19
session remains healthy. These are discrete KGlobalAccel actions; pointer,
button and scroll still travel over portal+libei.

As a negative capability test, stop the session and create a copy of the
settings that maps one gesture to an unsupported target such as
`notification-center` or `lookup`. Starting real M19 with that file must fail
before grab. During a running M19 session, changing a watched setting to the
same unsupported route must print `reload rejected` and retain last-good.
Restore a supported route and confirm the next valid generation loads.

## 4. Busy-boundary check

Hold/continue one interaction and edit the file from Terminal B. Terminal A
must report the generation as queued. No discontinuity or mid-gesture semantic
change is allowed. Release all fingers/buttons; the queued generation must then
apply exactly once.

## 5. Invalid-save recovery check

Using a text editor, intentionally create a temporarily invalid JSON save.
Terminal A must report `reload rejected` and continue using the previous
configuration. Restore a valid file; the next changed save must load normally.

Record the exact messages and whether pointer/scroll ownership remained stable.

Passing this procedure is still machine/session-specific live evidence. It is
not a macOS-equivalence claim and does not qualify unsupported KDE targets or
native continuous-gesture output.
