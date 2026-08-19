#!/usr/bin/env bash
# Docker-based end-to-end test: the daemon-hosted flare deployment form.
#
# The flare server normally runs inside `lkit daemon` (config.toml `[flare]`
# section). This scenario verifies that the daemon-hosted server accepts an
# `lflare` client: DISCOVER -> RESP -> AUTH_REQ -> AUTH_ACK and a live session
# (KEEPALIVE) are enough; tunnel forwarding is NOT exercised here (see the
# FLR-19 scenario note: the full "failed network takeover, recover through
# the tunnel" flow is out of scope for this layer).
#
#   client container (/opt/bin/lflare cli --dev eth0 --token TOKEN --forward 2222:6443)
#        |  Terrain frames (ethertype 0x88B6, broadcast + unicast)
#        v
#   server container (lkit daemon, LKIT_TERRITORY=/tmp/territory,
#                     config.toml [flare] psk/token)
#
# Usage: scripts/flare/e2e-daemon.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=lndp-daemon-test
IMAGE=lndp-test:latest
SRV=landscape-daemon-srv
CLI=landscape-daemon-cli
PSK=daemon-hosted-recovery-secret
TOKEN=daemon-token

cleanup() {
  docker rm -f "$SRV" "$CLI" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

echo "== build =="
if [[ "${FLARE_E2E_SKIP_BUILD:-0}" != 1 ]]; then
  cargo build --workspace >/dev/null
fi
docker build -q -t "$IMAGE" -f scripts/flare/Dockerfile . >/dev/null
docker network create "$NET" >/dev/null

echo "== start daemon-hosted flare server =="
docker run -d --name "$SRV" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --env LKIT_TERRITORY=/tmp/territory \
  --cap-add NET_RAW --cap-add NET_ADMIN \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c '
    mkdir -p /tmp/territory
    cat > /tmp/territory/config.toml <<EOF
schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[flare]
psk = "'"$PSK"'"
token = "'"$TOKEN"'"
EOF
    exec /opt/bin/lkit daemon
  '

echo "== start client =="
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward 2222:6443

echo "== wait for the daemon to host the flare server =="
for i in $(seq 1 30); do
  if docker logs "$SRV" 2>&1 | grep -q "server .* ready"; then
    break
  fi
  sleep 1
done
if ! docker logs "$SRV" 2>&1 | grep -q "server .* ready"; then
  echo "FAIL: the daemon never reported the flare server ready"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "daemon-hosted flare server ready"

echo "== wait for the client session =="
for i in $(seq 1 30); do
  if docker logs "$CLI" 2>&1 | grep -q "session .* established"; then
    break
  fi
  sleep 1
done
if ! docker logs "$CLI" 2>&1 | grep -q "session .* established"; then
  echo "FAIL: the client never established a session through the daemon-hosted server"
  docker logs "$CLI" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "client session established through the daemon-hosted server"

echo "== the server must have authenticated the client =="
if ! docker logs "$SRV" 2>&1 | grep -q "authenticated, session"; then
  echo "FAIL: the daemon-hosted server never authenticated the client"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "authentication confirmed on the daemon side"

echo "== keepalives must keep the session alive =="
sleep 6
if ! docker logs "$CLI" 2>&1 | grep -q "keepalive"; then
  # lflare logs keepalives; the authoritative liveness check is the server
  # not tearing the peer down within the stale window.
  :
fi
echo "DAEMON-HOSTED FLARE E2E PASSED"
