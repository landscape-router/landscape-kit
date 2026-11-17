#!/usr/bin/env bash
set -euo pipefail
[[ $(cat /etc/hostname) == lkit-qemu && -f /var/lib/lkit-qemu/password ]] || {
  echo "This helper requires the disposable lkit QEMU guest" >&2
  exit 2
}
[[ -e /var/lib/lkit-qemu/force-ra ]] || exit 0
wan=$(jq -r '.network_takeover.plan.mode.wan' /root/.lkit/transactions/*.json)
for _ in $(seq 1 60); do
  if ip -6 -o address show dev "$wan" | grep -q 'fec0:'; then
    touch /var/lib/lkit-qemu/ra-observed
    echo 'QEMU: IPv6 RA was configured before NetworkManager startup'
    exit 0
  fi
  sleep 1
done
echo 'QEMU: expected WAN IPv6 RA before NetworkManager startup' >&2
exit 1
