#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${PREFIX:-"$HOME/.local"}
CONFIG_HOME=${XDG_CONFIG_HOME:-"$HOME/.config"}
STATE_HOME=${XDG_STATE_HOME:-"$HOME/.local/state"}
SYSTEMD_USER_HOME=${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user
ENABLE=0
INSTALL_UDEV=${INSTALL_UDEV:-1}

for arg in "$@"; do
    case "$arg" in
        --enable) ENABLE=1 ;;
        --no-udev) INSTALL_UDEV=0 ;;
        *)
            echo "unknown argument: $arg" >&2
            echo "usage: packaging/install.sh [--enable] [--no-udev]" >&2
            exit 2
            ;;
    esac
done

cd "$ROOT"
cargo build --release --locked -p touchpadctl

install -d "$PREFIX/bin" "$CONFIG_HOME/touchpad2mac" "$STATE_HOME/touchpad2mac" "$SYSTEMD_USER_HOME"
install -m 0755 target/release/touchpadctl "$PREFIX/bin/touchpadctl"

if [ ! -e "$CONFIG_HOME/touchpad2mac/settings.json" ]; then
    install -m 0644 settings.json "$CONFIG_HOME/touchpad2mac/settings.json"
else
    echo "keeping existing $CONFIG_HOME/touchpad2mac/settings.json"
fi

install -m 0644 packaging/systemd/touchpad2mac.service "$SYSTEMD_USER_HOME/touchpad2mac.service"

if [ "$INSTALL_UDEV" -eq 1 ]; then
    if command -v sudo >/dev/null 2>&1; then
        sudo install -m 0644 packaging/udev/99-touchpad2mac.rules /etc/udev/rules.d/99-touchpad2mac.rules
        sudo udevadm control --reload-rules
        sudo udevadm trigger --subsystem-match=input || true
    else
        echo "warning: sudo not found; skipping udev rule installation" >&2
        echo "install packaging/udev/99-touchpad2mac.rules into /etc/udev/rules.d manually" >&2
    fi
fi

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
    if [ "$ENABLE" -eq 1 ]; then
        systemctl --user enable --now touchpad2mac.service
    fi
fi

cat <<EOF
touchpad2mac installed.

Binary:   $PREFIX/bin/touchpadctl
Settings: $CONFIG_HOME/touchpad2mac/settings.json
Service:  $SYSTEMD_USER_HOME/touchpad2mac.service

Run diagnostics:
  $PREFIX/bin/touchpadctl doctor $CONFIG_HOME/touchpad2mac/settings.json

Start manually:
  systemctl --user start touchpad2mac.service

Enable at login:
  systemctl --user enable --now touchpad2mac.service
EOF
