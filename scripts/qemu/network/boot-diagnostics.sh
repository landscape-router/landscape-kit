#!/usr/bin/env bash
# Read-only diagnostics remain reachable through serial when router networking fails.
set -uo pipefail
[[ -f /etc/lkit-qemu-network ]] || exit 2
sleep 10
echo '== QEMU postboot service diagnostics =='
cat /proc/sys/kernel/random/boot_id
findmnt /srv
ls -l /etc/systemd/system/landscape-router.service \
  /etc/systemd/system/multi-user.target.wants/landscape-router.service \
  /srv/landscape/service/landscape-router.service 2>&1 || true
systemctl show lkit.service landscape-router.service \
  -p Id -p LoadState -p LoadError -p ActiveState -p SubState -p UnitFileState \
  -p FragmentPath -p NeedDaemonReload
systemctl list-jobs --no-pager
journalctl -b --no-pager -n 80 -u lkit.service -u landscape-router.service
echo '== QEMU postboot service diagnostics complete =='
