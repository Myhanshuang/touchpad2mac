# Security policy

## Supported versions

Until the first stable release, only the latest `main` branch and latest
tagged prerelease receive security fixes.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting for this repository when
available. Do not open a public issue for a vulnerability that could allow
input injection, privilege escalation, unintended device access, or exposure
of private input data.

Include the affected commit/version, platform, reproduction conditions and a
minimal proof of concept. Avoid attaching real passwords, keyboard captures or
other secrets.

## Security boundaries

- The Linux service grabs only the selected physical touchpad.
- Disable-while-typing opens paired keyboards read-only and reduces relevant
  presses to anonymous timestamps; key codes are not written to touch traces
  or diagnostics bundles.
- The udev rule uses active-session `uaccess`; it does not make all input
  devices world-readable/writable.
- Portal/libei authorization remains under the desktop session's security
  model.
- Windows full takeover is not claimed until a separately reviewed, signed
  filter-driver boundary exists.
