#!/usr/bin/env bash
# Sourced by the disposable QEMU network harness. All router changes go through guest SSH.
# shellcheck disable=SC2154
[[ ${BASH_SOURCE[0]} != "$0" ]] || { echo 'Use the QEMU network harness' >&2; exit 2; }

prepare_flare_router() {
  wan_ssh 'test ! -e /srv/landscape/data/landscape.toml && /usr/local/bin/lkit self install'
  daemon_instance=$(wan_ssh 'systemctl show lkit.service -p InvocationID --value')
  router_id=$(wan_ssh 'lkit --lang en flare setup' | sed -n 's/^flare: device fingerprint: //p')
  [[ $router_id =~ ^[0-9a-f]{64}$ && $daemon_instance =~ ^[0-9a-f]{32}$ ]]
}

discover_flare_router() {
  "$lflare_binary" discover --dev "$active_tap" --json |
    jq -e --arg id "$router_id" 'map(.fingerprint) == [$id]' >/dev/null
}

open_flare_ssh() {
  local auth=$1 password_file=$2
  mapped_port=$(next_port)
  "$lflare_binary" cli --dev "$active_tap" --target "$router_id" --auth "$auth" \
    --password-file "$password_file" --forward "$mapped_port:22" >"$test_root/flare-client.log" 2>&1 &
  flare_client_pid=$!
  for _ in $(seq 1 35); do
    if ssh "${ssh_common[@]}" -o IdentitiesOnly=yes -p "$mapped_port" root@127.0.0.1 \
      'test -f /etc/lkit-qemu-network' >/dev/null 2>&1; then return; fi
    kill -0 "$flare_client_pid" 2>/dev/null || break
    sleep 1
  done
  cat "$test_root/flare-client.log" >&2
  return 1
}

close_flare_ssh() {
  if [[ -n ${flare_client_pid:-} ]]; then
    kill "$flare_client_pid" 2>/dev/null || true
    wait "$flare_client_pid" 2>/dev/null || true
    flare_client_pid=
  fi
}

run_flare_rollback() {
  local original=192.168.10.99 transaction deadline
  local daemon_instance router_id mapped_port
  boot_vm flare-rollback
  wan_ssh 'nmcli con mod lkit-lan-unconfigured ipv4.method manual ipv4.addresses 192.168.10.99/24 connection.autoconnect yes && nmcli con up lkit-lan-unconfigured'
  ssh "${ssh_common[@]}" "root@$original" 'test -f /etc/lkit-qemu-network'
  prepare_flare_router
  # Set a disposable guest shadow password. sshd continues to require a key.
  wan_ssh 'chpasswd' <<< 'root:FixtureRescue9'
  (umask 077; echo 'FixtureRescue9' >"$test_root/root-password")
  start_takeover flare-rollback
  if ssh "${ssh_common[@]}" "root@$original" true >/dev/null 2>&1; then
    echo 'Takeover left the original management address accessible' >&2; return 1
  fi
  discover_flare_router
  open_flare_ssh root "$test_root/root-password"
  local -a tunnel=(ssh "${ssh_common[@]}" -p "$mapped_port" root@127.0.0.1)
  transaction=$("${tunnel[@]}" "jq -er 'select(.phase == \"awaiting_network_confirmation\") | .transaction_id' /root/.lkit/transactions/*.json")
  deadline=$("${tunnel[@]}" "jq -er '.network_takeover.confirmation_deadline' /root/.lkit/transactions/*.json")
  # Rollback can tear down a link while the CLI is waiting for its operation.
  "${tunnel[@]}" 'nohup lkit network rollback </dev/null >/var/lib/lkit-qemu/flare-rollback.log 2>&1 &'
  local restored=false wait_until=$((SECONDS + 90))
  while ((SECONDS < wait_until)); do
    if ssh -o ConnectTimeout=2 "${ssh_common[@]}" "root@$original" 'test -f /etc/lkit-qemu-network' >/dev/null 2>&1; then restored=true; break; fi
    sleep 1
  done
  if [[ $restored != true ]]; then
    echo 'Original LAN SSH still unreachable after explicit rollback' >&2
    wan_ssh 'ip -4 route get 192.168.10.2 from 192.168.10.99; ip -4 route; cat /var/lib/lkit-qemu/flare-rollback.log' >&2 || true
    return 1
  fi
  ssh "${ssh_common[@]}" "root@$original" bash -s -- "$transaction" "$deadline" <<'GUEST'
set -euo pipefail
tx=/root/.lkit/transactions/$1.json
jq -e '.phase == "rolled_back"' "$tx" >/dev/null
(( $(date -d "$(jq -r .updated_at "$tx")" +%s) < $(date -d "$2" +%s) ))
for _ in $(seq 1 30); do
  if jq -se --arg id "$1" 'any(.[]; .kind == "network_rollback" and .network_transaction == $id and .status == "succeeded" and .exit_code == 0)' /root/.lkit/run/operations/*.json >/dev/null; then exit 0; fi
  sleep 1
done
exit 1
GUEST
  close_flare_ssh
  echo 'PASS: lost management IP restored by explicit rollback over Flare before the deadline'
}

run_flare_password() {
  local daemon_instance router_id mapped_port rotated="$test_root/rotated-password"
  boot_vm flare-password
  prepare_flare_router
  start_takeover flare-password
  lan_ssh 'lkit network confirm'
  journey_guest identity
  discover_flare_router
  open_flare_ssh landscape "$test_root/journey-password"
  close_flare_ssh
  (umask 077; echo 'RotatedLandscape9' >"$rotated")
  python3 - "$repo_root/scripts/qemu/network" "$test_root/journey-password" "$rotated" <<'PY'
from pathlib import Path
import sys
sys.path.insert(0, sys.argv[1])
from journey import Api
url = 'https://192.168.10.1:6443'
old, new = map(Path, sys.argv[2:])
password = new.read_text().strip()
Api(url, old).call('POST', '/api/v1/system/config/edit/auth', {
    'current_password': old.read_text().strip(), 'new_password': password, 'confirm_password': password,
})
Api(url, new).assert_web()
PY
  # shellcheck disable=SC2016
  lan_ssh 'systemctl stop landscape-router.service && test "$(systemctl is-active landscape-router.service)" = inactive'
  if timeout 25 "$lflare_binary" cli --dev "$active_tap" --target "$router_id" --auth landscape \
    --password-file "$test_root/journey-password" --forward "$(next_port):22" >"$test_root/rejected.log" 2>&1; then
    echo 'Old Landscape password still accepted' >&2; return 1
  fi
  grep -F 'authentication failed' "$test_root/rejected.log"
  open_flare_ssh landscape "$rotated"
  [[ $(ssh "${ssh_common[@]}" -p "$mapped_port" root@127.0.0.1 'systemctl show lkit.service -p InvocationID --value') == "$daemon_instance" ]]
  close_flare_ssh
  echo 'PASS: unchanged daemon uses API-rotated Landscape password with Web offline'
}
