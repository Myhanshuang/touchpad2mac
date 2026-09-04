#!/bin/sh
set -eu

PREFIX=${PREFIX:-"$HOME/.local"}
CONFIG_HOME=${XDG_CONFIG_HOME:-"$HOME/.config"}
SYSTEMD_USER_HOME=${XDG_CONFIG_HOME:-"$HOME/.config"}/systemd/user
REMOVE_CONFIG=0
REMOVE_UDEV=${REMOVE_UDEV:-1}

for arg in "$@"; do
    case "$arg" in
        --remove-config) REMOVE_CONFIG=1 ;;
        --keep-udev) REMOVE_UDEV=0 ;;
        *)
            echo "unknown argument: $arg" >&2
            echo "usage: packaging/uninstall.sh [--remove-config] [--keep-udev]" >&2
            exit 2
            ;;
    esac
done

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now touchpad2mac.service >/dev/null 2>&1 || true
fi

rm -f "$PREFIX/bin/touchpadctl" "$SYSTEMD_USER_HOME/touchpad2mac.service"

if [ "$REMOVE_CONFIG" -eq 1 ]; then
    rm -rf "$CONFIG_HOME/touchpad2mac"
else
    echo "keeping $CONFIG_HOME/touchpad2mac"
fi

if [ "$REMOVE_UDEV" -eq 1 ] && [ -e /etc/udev/rules.d/99-touchpad2mac.rules ]; then
    if command -v sudo >/dev/null 2>&1; then
        sudo rm -f /etc/udev/rules.d/99-touchpad2mac.rules
        sudo udevadm control --reload-rules
        sudo udevadm trigger --subsystem-match=input || true
    else
        echo "warning: remove /etc/udev/rules.d/99-touchpad2mac.rules manually" >&2
    fi
fi

if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
fi

echo "touchpad2mac runtime files removed"
