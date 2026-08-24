# M16 Capability Matrix

This matrix mirrors `touchpad_core::production::capability_matrix()` and is a
qualification statement, not a feature wishlist.

| Capability | Current status | Boundary |
|---|---|---|
| Wayland RemoteDesktop + libei | experimental / unqualified | Implemented; needs M6/M10–M16 live evidence |
| X11 adapter | separate qualification | No implementation or silent fallback |
| uinput adapter | separate qualification | No implementation or silent fallback |
| Continuous gestures | semantic-only | Core recognition exists; current M6 sink rejects native continuous events |
| KDE desktop actions | semantic-only | Configurable injected adapter exists; real transport disabled |
| Pressure | unsupported | No qualified feature on current hardware/profile |
| Haptics | unsupported | No qualified haptic hardware/output interface |

`m16-production-v1` means the configuration/operational contracts are present;
it does **not** mean cross-device or live-production qualification.
