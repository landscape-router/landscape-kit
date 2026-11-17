#!/usr/bin/env bash
# Runs only in the disposable QEMU guest.
set -euo pipefail
[[ $(cat /etc/hostname) == lkit-qemu && -f /var/lib/lkit-qemu/password ]] || {
  echo "This helper requires the disposable lkit QEMU guest" >&2
  exit 2
}
mkdir -p /etc/systemd/system/NetworkManager.service.d
cat >/etc/systemd/system/NetworkManager.service.d/lkit-ci.conf <<'UNIT'
[Service]
ExecStartPre=/bin/bash /var/lib/lkit-qemu/await-ra.sh
UNIT
cat >/etc/systemd/system/lkit-qemu-recovery-check.service <<'UNIT'
[Unit]
Description=Verify failed connection recovery remains retryable
After=lkit.service
[Service]
Type=oneshot
ExecStart=/bin/bash /var/lib/lkit-qemu/recovery-check.sh
TimeoutStartSec=240
[Install]
WantedBy=multi-user.target
UNIT
cat >/etc/systemd/system/lkit-qemu-boot-diagnostics.service <<'UNIT'
[Unit]
Description=Capture read-only router boot diagnostics over serial
After=local-fs.target
[Service]
Type=oneshot
ExecStart=/bin/bash /var/lib/lkit-qemu/boot-diagnostics.sh
StandardOutput=journal+console
StandardError=journal+console
[Install]
WantedBy=multi-user.target
UNIT
systemctl enable lkit-qemu-boot-diagnostics.service
vendor=$(systemctl show --property=FragmentPath --value NetworkManager.service)
for kind in absolute relative; do
  target=lkit-qemu-$kind.target
  printf '[Unit]\nDescription=Custom host network startup target\n' >"/etc/systemd/system/$target"
  mkdir -p "/etc/systemd/system/$target.wants"
  link=$vendor
  [[ $kind != relative ]] || link=../NetworkManager.service
  ln -s "$link" "/etc/systemd/system/$target.wants/NetworkManager.service"
done
printf '%s\n' "$vendor" >/var/lib/lkit-qemu/network-manager-vendor
systemctl daemon-reload
