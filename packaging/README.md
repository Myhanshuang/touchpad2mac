# Packaging and installation

The supported Linux installation layout is intentionally user-scoped except
for the udev access rule:

- binary: `~/.local/bin/touchpadctl`
- settings: `~/.config/touchpad2mac/settings.json`
- persistent state/traces: `~/.local/state/touchpad2mac/`
- user service: `~/.config/systemd/user/touchpad2mac.service`
- udev rule: `/etc/udev/rules.d/99-touchpad2mac.rules`

Install without enabling the service:

```bash
./packaging/install.sh
```

Install and enable immediately:

```bash
./packaging/install.sh --enable
```

The udev rule uses `TAG+="uaccess"`; it does **not** make input devices
world-readable or world-writable. Pass `--no-udev` when a distribution or
administrator already supplies the required active-session ACLs.

Uninstall runtime files while preserving user settings:

```bash
./packaging/uninstall.sh
```

Use `--remove-config` only when the local settings should be deleted as well.
