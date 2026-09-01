# Windows port status

The Windows port deliberately distinguishes **overlay/probe support** from a
true **physical touchpad takeover**. This distinction is a correctness and
safety requirement, not a missing label.

## Implemented now

`crates/touchpad-windows` provides:

1. Precision Touchpad discovery from the Win32 Raw Input device list.
   Devices are accepted only when their HID top-level collection is
   Digitizers / Touch Pad (`usage page 0x0D`, `usage 0x05`).
2. A read-only support probe exposed as:

   ```powershell
   target\release\touchpadctl.exe windows-probe
   ```

   It reports discovered PTP devices, Raw Input availability, `SendInput`
   availability, native synthetic-touchpad API exports, and the full-takeover
   blocker.
3. A testable `WindowsOutputSink` that maps `touchpad-core::OutputEvent` to a
   mockable Windows output API. The real Windows implementation uses
   `SendInput` for relative pointer movement, Left/Right/Middle/X1/X2 button
   edges, vertical wheel, and horizontal wheel.
4. Held-button tracking and idempotent `release_all`, matching the core output
   contract instead of emitting unmatched button-up events.
5. Dynamic probing of the recent Windows 11 synthetic Precision Touchpad API
   exports (`CreateSyntheticPointerDevice2`, `InjectSyntheticPointerInput`,
   `InjectTouchpadAction`, `DestroySyntheticPointerDevice`). They are not
   linked as mandatory imports, so older Windows builds can report them as
   unavailable cleanly.

## Why full takeover is intentionally blocked

Linux takeover relies on `EVIOCGRAB`: the daemon consumes the physical
touchpad and the desktop no longer receives the original evdev stream. The
current Windows user-mode APIs do not provide an equivalent for a Precision
Touchpad HID collection.

Raw Input is useful for background HID observation, but its `RIDEV_NOLEGACY`
suppression behavior applies to the mouse/keyboard legacy message path. It is
not a general user-mode PTP grab. Running our own pointer output while Windows
continues to process the physical PTP would therefore risk duplicate movement,
scrolling, clicking, or gestures.

For that reason the Windows port currently reports:

```text
user-mode full takeover: unavailable
```

The next required Windows-only component for feature parity is a signed HID
or mouse-class filter driver that can suppress/redirect the selected physical
PTP while preserving fail-open behavior. Only after that boundary exists
should the Raw Input/HID contact decoder be connected to the existing
`touchpad-core` arbiter for system-wide replacement.

## Precision Touchpad report model for the next input stage

Microsoft's PTP HID contract gives us a stable decoder target rather than a
vendor-specific packet format. The mandatory contact-level usages are:

- Contact ID: Digitizers `0x51`
- X: Generic Desktop `0x30`
- Y: Generic Desktop `0x31`
- Tip switch: Digitizers `0x42`
- Confidence: Digitizers `0x47`

Mandatory report-level usages include Scan Time (`0x56`) and Contact Count
(`0x54`). Width/Height, Pressure, and Azimuth are optional. Hybrid reporting
is legal, so a Windows decoder must assemble all reports belonging to one scan
before publishing one normalized core frame.

That decoder should preserve the same project invariants already used on
Linux: stable contact IDs, monotonic frame timestamps, atomic physical-button
state, no partial frame publication, and fail-closed behavior on malformed or
incomplete reports.

## Validation status

The platform-neutral Windows code and output state machine are covered by
Linux-hosted unit tests and the full workspace gates. The actual `win32.rs`
FFI block is `cfg(target_os = "windows")` and therefore requires a Windows
toolchain/runner for ABI and live-device qualification. Until those live tests
are performed, Windows remains experimental and must not be described as a
drop-in replacement for the Linux takeover path.
