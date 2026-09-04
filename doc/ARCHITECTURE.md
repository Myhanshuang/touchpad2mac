# Architecture

touchpad2mac is deliberately split at two typed boundaries so platform and
desktop support can evolve without duplicating gesture policy.

```text
physical / virtual touchpad
        |
        v
platform input adapter
  Linux evdev Type-B
  Windows PTP (in progress)
        |
        v
ContactFrame
        |
        v
touchpad-core interaction arbiter
        |
        +-- pointer / click / tap-drag
        +-- two-finger scroll
        +-- three-finger drag / tap
        +-- continuous gestures
        +-- palm/thumb/DWT robustness
        |
        v
OutputEvent
        |
        +-- portal + libei pointer/button/scroll
        +-- KDE semantic desktop actions
        +-- Windows semantic output (experimental)
```

## Input boundary

Platform code is responsible for hardware enumeration, raw report decoding,
resynchronization and physical-unit normalization. Once a `ContactFrame`
exists, no recognizer may depend on Linux event codes, Windows HID usages, or
device file descriptors.

The Linux live boundary is:

```text
/dev/input/event*
  -> EvdevRuntime
  -> TypeBDecoder
  -> ContactFrame
```

Raw trace replay drives the exact same decoder. The optional
`touchpad-testkit` goes one level lower and creates a real `/dev/uinput`
device so CI can exercise the kernel input path before `EvdevRuntime`.

## Interaction ownership

Recognizers are competitors over one physical contact sequence. The safety
rule is:

1. candidate recognition is output-free;
2. only a committed recognizer may emit semantic output;
3. once committed, it owns the contact cluster until its defined termination;
4. lower-priority recognizers must not reinterpret the tail of an owned
   cluster;
5. cancellation/failure releases all synthetic held state.

This is why three-finger tap and three-finger drag share one classifier:
three-finger tap is emitted only after the sequence conclusively ends as a
tap. A committed drag can never later produce a middle click.

## Output boundary

`touchpad-core` emits typed `OutputEvent`s. It never calls D-Bus, libei,
Win32, shell commands or compositor APIs directly. Desktop adapters decide
whether a semantic event is supported and must fail before takeover when a
required mapping is impossible.

Held button/scroll lifecycle is tracked independently from gesture state so
cleanup can be idempotent even after partial output failure.

## Production and developer modes

Both modes use the same input/output/arbiter/cleanup implementation:

- `touchpadctl takeover ...` is the explicit developer/qualification path:
  mandatory trace, visible countdown and a maximum 300-second session.
- `touchpadctl service-run SETTINGS` is the packaged persistent path: no
  artificial deadline/countdown and no unbounded raw touch recording.

The systemd service restarts transient stream faults, but does not loop on
configuration, missing prerequisite or authorization failures that require an
operator action.

## Hardware quirks

Hardware-specific corrections are data in `quirks/builtin.json`, validated by
the strict schema in `touchpad-core::quirks`. Avoid scattered model-name
branches. Unknown devices stay on the generic profile rather than inheriting
an unproven correction.

## Privacy boundaries

Disable-while-typing opens eligible keyboards read-only and reduces relevant
key activity to anonymous monotonic timestamps at the Linux boundary. Key
codes do not enter `touchpad-core`, touch traces, or diagnostics bundles.

`touchpadctl diagnostics` contains static device/session metadata only. Raw
touch traces are separate, explicit artifacts created by `record` or the
developer `takeover` command.
