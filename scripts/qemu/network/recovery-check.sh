#!/usr/bin/env bash
set -euo pipefail
[[ $(cat /etc/hostname) == lkit-qemu && -f /var/lib/lkit-qemu/password ]] || {
  echo "This helper requires the disposable lkit QEMU guest" >&2
  exit 2
}
[[ -e /var/lib/lkit-qemu/force-ra ]] || exit 0
ready=false
for _ in $(seq 1 180); do
  if [[ -e /var/lib/lkit-qemu/ra-observed ]] \
    && systemctl is-active --quiet NetworkManager \
    && journalctl -b -u lkit.service --no-pager | grep 'NetworkManager recovery:' >/dev/null; then
    ready=true
    break
  fi
  sleep 1
done
[[ $ready == true ]]
jq -e '.phase == "rolling_back"' /root/.lkit/transactions/*.json >/dev/null
sleep 3
jq -e '.phase == "rolling_back"' /root/.lkit/transactions/*.json >/dev/null
pending=$(jq -r '.network_takeover.pending_state' /root/.lkit/transactions/*.json)
test -f "/root/.lkit/$pending"
touch /var/lib/lkit-qemu/retry-checked
cp /var/lib/lkit-qemu/wan.saved /etc/NetworkManager/system-connections/wan.nmconnection
chmod 0600 /etc/NetworkManager/system-connections/wan.nmconnection
nmcli connection reload
echo 'QEMU: restored missing profile; daemon must retry activation without manual connection up'
