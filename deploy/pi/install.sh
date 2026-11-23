#!/bin/sh
# Install the surfcult updater on the Raspberry Pi. Run as root from this directory. Safe to rerun.
set -eu

[ "$(id -u)" = 0 ] || { echo "run as root: sudo $0" >&2; exit 1; }
here=$(cd "$(dirname "$0")" && pwd)

install -m 0755 "$here/surfcult-update" /usr/local/sbin/surfcult-update
install -m 0644 "$here/surfcult-updater.service" "$here/surfcult-updater.timer" /etc/systemd/system/
install -d -m 0755 /opt/surfcult /opt/surfcult/releases
install -d -m 0700 /etc/surfcult-updater
systemctl daemon-reload
systemctl enable --now surfcult-updater.timer

if [ -s /etc/surfcult-updater/token ]; then
  echo "Installed. The timer runs every 10 minutes; run it now with: systemctl start surfcult-updater"
else
  echo "Installed, but there is no token yet, so nothing will update."
  echo "Add the read-only GitHub token (do not put it on a command line):"
  echo "  sudo install -m 600 -o root -g root /dev/stdin /etc/surfcult-updater/token"
fi
