# Contributing to touchpad2mac

Thanks for helping improve touchpad2mac. Input software has an unusually high
cost for regressions, so contributions are expected to preserve ownership,
cleanup and privacy invariants rather than only making a happy-path demo work.

## Development setup

Use Rust 1.87 or newer. Before opening a pull request run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Linux system tests use a real virtual kernel input device and are opt-in:

```bash
sudo modprobe uinput
sudo chmod a+rw /dev/uinput
TOUCHPAD2MAC_RUN_UINPUT=1 cargo test -p touchpad-testkit --test uinput_smoke --locked
```

## Architecture rule

Platform code normalizes hardware input into `ContactFrame`. Gesture and
ownership policy belongs in `touchpad-core`; desktop/platform adapters emit
typed `OutputEvent`s. Do not add a second platform-specific gesture engine.

Gesture recognizers must obey the ownership model: candidate recognition is
output-free, a committed owner suppresses competitors for the remainder of
the contact cluster, and cleanup must release all synthetic held state.

## Hardware support

Do not add product-name `if` statements throughout the runtime. Hardware
adjustments belong in `quirks/builtin.json` and must include evidence.

For a new machine, attach:

```bash
touchpadctl diagnostics diagnostics.json
touchpadctl qualify qualification.json
```

Complete the qualification checklist on real hardware. Do not attach private
keyboard input or unrelated logs. The diagnostics command deliberately does
not collect key codes or touch traces.

## Pull requests

- Keep commits focused and explain behavior changes.
- Add a regression test for bug fixes.
- Update user-facing documentation for settings/CLI changes.
- Do not weaken `EVIOCGRAB`, held-button cleanup or fail-closed validation to
  make a test pass.
- New `unsafe` code requires a documented safety boundary and focused tests.

Small, reviewable pull requests are preferred over unrelated feature bundles.
