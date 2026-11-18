#!/usr/bin/env bash
# CI only. Three network namespaces, one lab-owned Ethernet broadcast domain.
set -euo pipefail
cd "$(dirname "$0")/../.."
export FLARE_LAB_BIN="$PWD/target/x86_64-unknown-linux-musl/debug"
topology=scripts/flare/containerlab/rescue.clab.yml
a=clab-flare-rescue-a
b=clab-flare-rescue-b
client=clab-flare-rescue-client

# Use an isolated runner and refuse to take ownership of any existing bridge.
if ip link show lan >/dev/null 2>&1; then echo 'lab bridge already exists' >&2; exit 1; fi
sudo ip link add lan type bridge
cleanup() {
  sudo -E containerlab destroy -t "$topology" --cleanup || true
  sudo ip link del lan || true
}
trap cleanup EXIT
sudo ip link set lan up
docker build -q -t lkit-flare-lab:ci -f scripts/flare/Dockerfile .
sudo -E containerlab deploy -t "$topology"
for router in "$a" "$b"; do
  docker exec "$router" sh -ec '
    umask 077; mkdir -p /root/.lkit /tmp/www
    printf "[flare]\ndevices = \"eth1\"\ndevice_name = \"router\"\n" > /root/.lkit/config.toml
    ! command -v sshd
  '
  docker exec -d "$router" sh -c 'exec /opt/bin/lkit daemon >/tmp/daemon.log 2>&1'
done
docker exec -i "$a" chpasswd <<< 'root:fixture-root-password'
docker exec -i "$b" chpasswd -c sha512 <<< 'root:fixture-root-password'
docker exec "$a" sh -c 'echo A > /tmp/www/index.html'
docker exec "$b" sh -c 'echo B > /tmp/www/index.html'
docker exec -d "$a" python3 -m http.server 6443 --directory /tmp/www --bind 127.0.0.1
docker exec -d "$b" busybox httpd -f -p 127.0.0.1:6443 -h /tmp/www

fingerprint() {
  local text
  for _ in $(seq 1 40); do
    text=$(docker exec "$1" /opt/bin/lkit flare setup | sed -n 's/^flare: device fingerprint: //p')
    if [[ $text =~ ^[0-9a-f]{64}$ ]]; then printf '%s\n' "$text"; return; fi
    sleep 1
  done
  docker exec "$1" cat /tmp/daemon.log >&2
  return 1
}
scan() { docker exec "$client" /opt/bin/lflare discover --dev eth1 --json "$@"; }
request() {
  docker exec "$client" python3 -c 'import urllib.request; print(urllib.request.urlopen("http://127.0.0.1:26443",timeout=2).read().decode().strip())'
}
stop_client() {
  docker exec "$client" sh -c 'if test -f /tmp/client.pid; then kill "$(cat /tmp/client.pid)" 2>/dev/null || :; fi'
  for _ in $(seq 1 25); do
    if ! docker exec "$client" sh -c 'kill -0 "$(cat /tmp/client.pid)" 2>/dev/null'; then return; fi
    sleep .2
  done
  return 1
}
start_client() {
  docker exec -d "$client" sh -c 'echo $$ >/tmp/client.pid; exec /opt/bin/lflare cli --dev eth1 --forward 26443:6443 "$@" >/tmp/client.log 2>&1' sh "$@"
}
expect_target() {
  local want=$1; shift
  start_client "$@"
  for _ in $(seq 1 45); do
    if [[ $(request 2>/dev/null) == "$want" ]]; then return; fi
    sleep 1
  done
  docker exec "$client" cat /tmp/client.log >&2
  return 1
}
reject() {
  if docker exec "$client" timeout 20 /opt/bin/lflare cli --dev eth1 --forward 26443:6443 "$@" >/tmp/flare-lab-reject.log 2>&1; then
    echo 'unexpected login success' >&2; return 1
  fi
  grep -Eq 'authentication failed|selected fingerprint not found' /tmp/flare-lab-reject.log
  if request >/dev/null 2>&1; then echo 'unexpected HTTP mapping after rejection' >&2; return 1; fi
}
a_id=$(fingerprint "$a")
b_id=$(fingerprint "$b")
test "$a_id" != "$b_id"
root=(--auth root --password-file /opt/fixture-password)
# Both replies have the same display name. Reorder arrivals and verify the data.
sudo tc qdisc add dev fr-b root netem delay 75ms
scan | jq -e --arg a "$a_id" --arg b "$b_id" 'length == 2 and all(.[]; .name == "router") and ([.[].fingerprint]|sort) == ([$a,$b]|sort)'
for router in "$a" "$b"; do
  if docker exec "$router" grep -q 'authenticated, session' /tmp/daemon.log; then exit 1; fi
done
expect_target B --target "$b_id" "${root[@]}"
if docker exec "$a" grep -q 'authenticated, session' /tmp/daemon.log; then exit 1; fi
stop_client
sudo tc qdisc del dev fr-b root
expect_target A --target "$a_id" "${root[@]}"
# Verify delivery after loss through the v6 data path.
docker exec "$a" tc qdisc add dev eth1 root netem loss 2%
for _ in $(seq 1 10); do test "$(request)" = A; done
docker exec "$a" tc qdisc del dev eth1 root
stop_client
docker exec "$client" sh -ec 'umask 077; echo wrong-password > /tmp/wrong'
reject --target "$a_id" --auth root --password-file /tmp/wrong
# Lock root on the Alpine server. Existing Landscape credentials are independent.
docker exec "$b" passwd -l root
reject --target "$b_id" "${root[@]}"

# Model a late-mounted managed install using the same root/state layout as lkit.
# The HTTP test service is an independent destination; no Landscape web API runs here.
pid_before=$(docker exec "$b" cat /root/.lkit/run/lkit.pid)
docker exec "$b" mkdir -p /srv/landscape/data /root/.lkit/state
docker cp scripts/flare/containerlab/state.json "$b:/root/.lkit/state/install-state.json"
set_landscape_password() {
  docker exec -i "$b" sh -ec 'umask 077; cat > /srv/landscape/data/landscape.toml.new; mv /srv/landscape/data/landscape.toml.new /srv/landscape/data/landscape.toml'
}
set_landscape_password <<< $'[auth]\nadmin_user = "operator"\nadmin_pass = "first-pass"'
docker exec "$client" sh -ec 'umask 077; echo first-pass >/tmp/first; echo new-pass >/tmp/current'
landscape=(--target "$b_id" --auth landscape --user operator)
expect_target B "${landscape[@]}" --password-file /tmp/first
stop_client
set_landscape_password <<< $'[auth]\nadmin_user = "operator"\nadmin_pass = "new-pass"'
reject "${landscape[@]}" --password-file /tmp/first
expect_target B "${landscape[@]}" --password-file /tmp/current
stop_client
test "$(docker exec "$b" cat /root/.lkit/run/lkit.pid)" = "$pid_before"

# Private discovery, target disappearance, and public A remaining reachable.
key=fixture-discovery-key
docker exec "$b" /opt/bin/lkit flare setup --psk "$key"
for _ in $(seq 1 30); do
  if scan | jq -e --arg a "$a_id" 'map(.fingerprint) == [$a]' >/dev/null; then break; fi
  sleep 1
done
scan | jq -e --arg a "$a_id" 'map(.fingerprint) == [$a]'
scan --psk invalid-discovery-key | jq -e 'length==0'
expect_target B "${landscape[@]}" --psk "$key" --password-file /tmp/current
stop_client
docker stop "$b" >/dev/null
reject --target "$b_id" "${root[@]}"
expect_target A --target "$a_id" "${root[@]}"
stop_client

# SSH user authentication must take place inside an already authenticated Flare mapping.
docker exec "$a" sh -ec 'apt-get update -qq; apt-get install -y -qq openssh-server; mkdir -p /run/sshd /root/.ssh; chmod 700 /root/.ssh'
docker exec "$client" sh -ec 'apt-get update -qq; apt-get install -y -qq openssh-client; ssh-keygen -q -t ed25519 -N "" -f /tmp/testkey'
docker exec "$client" cat /tmp/testkey.pub | docker exec -i "$a" sh -ec 'umask 077; cat >/root/.ssh/authorized_keys'
docker exec -d "$a" /usr/sbin/sshd -D -o ListenAddress=127.0.0.1 -o PasswordAuthentication=no -o PermitRootLogin=prohibit-password
expect_target A --target "$a_id" "${root[@]}" --forward 2222:22
ssh_opts=(-o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=3 -p 2222)
docker exec "$client" ssh "${ssh_opts[@]}" -i /tmp/testkey root@127.0.0.1 true
if docker exec "$client" ssh "${ssh_opts[@]}" -o IdentityAgent=none -o IdentitiesOnly=yes root@127.0.0.1 true >/tmp/flare-lab-ssh.log 2>&1; then exit 1; fi
grep -Fi 'Permission denied' /tmp/flare-lab-ssh.log
docker exec "$a" sh -ec 'kill "$(cat /run/sshd.pid)"'
test "$(request)" = A
stop_client

# Run the actual pinned v5 daemon, then verify legacy transfer and upgrade notice.
docker exec "$a" sh -c 'kill "$(cat /root/.lkit/run/lkit.pid)"'
docker cp target/debug/lkit-v5 "$a:/tmp/lkit-v5"
docker exec -d "$a" /tmp/lkit-v5 flare serve --dev eth1 --psk fixture-v5-secret
expect_target A --legacy --psk fixture-v5-secret
docker exec "$client" grep -F 'upgrade lkit daemon' /tmp/client.log
stop_client
echo 'containerlab Flare interoperability passed'
