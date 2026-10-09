#!/usr/bin/env bash
# Bridge double-capture end-to-end test.
#
# The daemon-managed flare server binds --dev any by default. On a router
# whose LAN ports Landscape bridges into br_lan, the same broadcast DISCOVER
# is then captured twice in one server process — once on the physical slave
# and once on the bridge master — and before the discover_id idempotency
# fix the second capture reminted the pending server nonce after the client
# had committed to the first RESP, looping the handshake forever between
# "discover ok" and "auth timeout".
#
# Topology (the bridge lives INSIDE the server container, mirroring br_lan
# on the router):
#
#   client container (eth0) ── docker L2 bridge ── server container
#                                                      ├─ eth0 (bridge port)
#                                                      └─ br0 (bridge master)
#     server: lkit flare serve --dev any → one broadcast, two captures
#
# Steps:
#   1. build br0 inside the server container, enslave eth0, adopt its MAC,
#      start serve with --dev any
#   2. the client must establish a session (deadlocks before the fix)
#   3. the server log shows the duplicated "discover from" lines (the
#      double capture is real, the test is not passing vacuously) and
#      exactly one authenticated session
#   4. a 2 MiB transfer through the tunnel is intact
#
# Usage: test/e2e-double-capture.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=lndp-dbl
IMAGE=lndp-test:latest
SRV=landscape-dbl-srv
CLI=landscape-dbl-cli
PSK=secret
TOKEN=lndp-token

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

echo "== start server (eth0 enslaved into an in-container br0, --dev any) =="
# br0 adopts eth0's MAC explicitly: the transport stamps frames with the
# sending interface's MAC, and the client filters AUTH frames by the source
# MAC of the RESP it accepted — br0 and eth0 must answer with one identity.
docker run -d --name "$SRV" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW --cap-add NET_ADMIN \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c '
    ip link add br0 type bridge
    ip link set dev br0 address "$(cat /sys/class/net/eth0/address)"
    ip link set eth0 master br0
    ip link set br0 up
    python3 /opt/fake_service.py &
    exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any --token '"$TOKEN"

echo "== start client =="
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward 2222:6443

wait_session() {
  local tries=45
  for i in $(seq 1 "$tries"); do
    if [ "$(docker logs "$CLI" 2>&1 | grep -c "session .* established" || true)" -ge 1 ]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

if ! wait_session; then
  echo "FAIL: the client never established a session under double capture"
  echo "== client log =="
  docker logs "$CLI" 2>&1 | tail -20
  echo "== server log =="
  docker logs "$SRV" 2>&1 | tail -30
  exit 1
fi
echo "session established under double capture"

echo "== confirm the double capture actually happened =="
# The accepted-discover line carries the quoted client name; the dropped or
# rate-limited variants do not, so this counts only real handshakes.
DISCOVER_LINES=$(docker logs "$SRV" 2>&1 | grep -c "discover from .* '" || true)
AUTH_LINES=$(docker logs "$SRV" 2>&1 | grep -c "authenticated, session" || true)
if [ "$DISCOVER_LINES" -lt 2 ]; then
  echo "FAIL: only $DISCOVER_LINES discover line(s) — the environment did not reproduce the double capture, this run proves nothing"
  docker logs "$SRV" 2>&1 | tail -30
  exit 1
fi
if [ "$AUTH_LINES" -ne 1 ]; then
  echo "FAIL: expected exactly 1 authenticated session, saw $AUTH_LINES"
  docker logs "$SRV" 2>&1 | tail -30
  exit 1
fi
echo "double capture confirmed ($DISCOVER_LINES discover lines) with one authenticated session"

echo "== transfer through the bridge-captured server =="
out=$(docker exec "$CLI" bash -c '
  set -o pipefail
  dd if=/dev/urandom of=/tmp/in.bin bs=1M count=2 status=none || exit 1
  md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
  nc -q 5 -w 60 127.0.0.1 2222 < /tmp/in.bin > /tmp/out.bin
  md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
  if cmp -s /tmp/in.md5 /tmp/out.md5; then
    echo "OK ($(wc -c < /tmp/out.bin) bytes)"
  else
    echo "MISMATCH: $(cat /tmp/in.md5) vs $(cat /tmp/out.md5)"
  fi
')
echo "$out"
if [[ "$out" != OK* ]]; then
  echo "== client log tail ($CLI) =="
  docker logs "$CLI" 2>&1 | tail -30 || true
  echo "== server log tail ($SRV) =="
  docker logs "$SRV" 2>&1 | tail -40 || true
  exit 1
fi

echo "ALL DOUBLE-CAPTURE E2E TESTS PASSED"
