# Hardware support and qualification

Hardware support is evidence-driven. A device being recognized as Type-B
multitouch does not by itself mean its palm thresholds, reported resolution,
firmware behavior or suspend/resume lifecycle have been qualified.

## Before reporting a hardware problem

Run:

```bash
touchpadctl doctor ~/.config/touchpad2mac/settings.json
touchpadctl diagnostics diagnostics.json
touchpadctl qualify qualification.json
```

`diagnostics.json` intentionally contains no keyboard key codes and no raw
touch trace. Inspect it before uploading if desired.

Complete each real-hardware test in `qualification.json` by changing its
`status` from `untested` to `pass`, `fail`, or `not-applicable`, and add short
notes for failures. Do not mark a case passed from an offline unit test.

## Qualification cases

The generated checklist covers:

- pointer motion;
- single/double tap and tap-drag;
- two-finger scrolling;
- three-finger middle click;
- three-finger drag and the tap/drag boundary;
- disable-while-typing;
- palm rejection;
- suspend/resume;
- hot unplug/replug;
- controlled SIGTERM cleanup.

For gesture bugs that need raw evidence, create a short explicit trace with
`record`/`takeover` and reproduce only the problematic gesture. Raw touch
traces may reveal physical interaction patterns and therefore are never
silently bundled by `diagnostics`.

## Adding a quirk

Quirks live in `quirks/builtin.json`. An entry may match exact vendor/product
IDs and/or a case-insensitive kernel device-name substring. Every populated
predicate must match, and the first matching entry wins.

Use quirks only for observed hardware facts/corrections. Do not use a quirk to
hide a recognizer bug that affects generic devices.

Each hardware-support pull request should include:

1. `diagnostics.json`;
2. completed `qualification.json`;
3. the smallest quirk entry needed, when one is required;
4. a regression test for the quirk matcher/schema;
5. a short explanation of the evidence supporting each correction.

## Support tiers

The project uses these practical tiers:

- **recognized** — passes the generic device candidate rules;
- **community-tested** — a user supplied a completed qualification report;
- **qualified** — the full checklist passed on a named hardware/kernel/
  desktop combination and no unresolved high-severity interaction bug is
  known for that combination.

The README does not claim broad hardware qualification until enough real
machines have produced evidence. CI can prove software invariants and the
virtual kernel path; it cannot substitute for physical firmware testing.
