# M18 Acceptance — Configurable Gesture Mapping

Status: **written, not executed live**. M18 is code-only/live-unqualified until
the user records this acceptance. The current portal/libei output path does not
by itself qualify real KDE `DesktopAction` execution; that transport must be
qualified separately.

## 1. Offline gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --release --workspace --locked
```

All must pass before any live test.

## 2. Settings-only acceptance (safe)

```bash
touchpadctl settings-default /tmp/touchpad-default.json
touchpadctl settings-macos /tmp/touchpad-macos.json
touchpadctl settings-check /tmp/touchpad-macos.json
touchpadctl settings-set /tmp/touchpad-macos.json /tmp/touchpad-custom.json \
  gesture.three-finger-swipe-up=open-overview \
  gesture.three-finger-swipe-down=show-desktop
touchpadctl settings-check /tmp/touchpad-custom.json
touchpadctl settings-gui /tmp/touchpad-custom.json /tmp/touchpad-settings.html
```

Confirm the HTML is local/self-contained and exports a settings file that
`settings-check` accepts.

## 3. Offline semantic acceptance

Required automated evidence:

- default M18 continuous gestures match M17 passthrough behavior;
- mapped gesture emits exactly one typed action on Begin;
- mapped Update/End are suppressed;
- disabled mapping emits nothing;
- three-finger tap is remappable and the generic M18 default remains Lookup;
- after the M19 real-KDE integration, `settings-macos` enables only the
  executable KDE subset and disables unsupported page/notification/lookup/
  native-continuous routes;
- the macOS-inspired preset disables three-finger drag commit, making
  three-finger swipes reachable; explicit three-finger-tap mappings remain
  functional even while drag commit is disabled;
- no arbitrary command string or shell execution exists in the schema.

## 4. Future live acceptance

Only after the output/action transports are explicitly qualified, run the
normal bounded takeover sequence with an external keyboard/mouse and a second
terminal available. Start with 10 seconds, then 60, then at most 300 seconds.

Do **not** mark M18 live-qualified merely because pointer/scroll output works:
each mapped desktop action must be observed and recorded separately.
