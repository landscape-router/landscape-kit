#!/usr/bin/env bash
# Multi-client end-to-end test: one server, two L2 segments, two clients.
#
#   client A (net A)  <--->  server (eth0 on net A, eth1 on net B)  <--->  client B (net B)
#
# The server runs with --dev any, so it receives LNDP frames on both
# interfaces and replies on the interface each client is attached to.
#
# Steps:
#   1. both clients establish their own encrypted session
#   2. concurrent transfers through both tunnels are intact
#   3. teardown of one client does not disturb the other's session
#   4. an idle session outlives the stale timeout (keepalive echoes) and
#      transfers again
#
# Usage: test/e2e-multiclient.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

NET_A=lndp-mc-a
NET_B=lndp-mc-b
IMAGE=lndp-test:latest
SRV=landscape-mc-srv
CLI_A=landscape-mc-a
CLI_B=landscape-mc-b
PSK=secret

cleanup() {
  docker rm -f "$SRV" "$CLI_A" "$CLI_B" >/dev/null 2>&1 || true
  docker network rm "$NET_A" "$NET_B" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

echo "== build =="
cargo build --workspace >/dev/null
docker build -q -t "$IMAGE" -f scripts/flare/Dockerfile . >/dev/null
docker network create "$NET_A" >/dev/null
docker network create "$NET_B" >/dev/null

echo "== start server on both networks =="
docker run -d --name "$SRV" --network "$NET_A" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c 'python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any'
docker network connect "$NET_B" "$SRV"
sleep 1

start_client() {
  local name=$1 net=$2 port=$3
  docker run -d --name "$name" --network "$net" \
    --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
    --cap-add NET_RAW \
    -v "$PWD/target/debug:/opt/bin:ro" \
    "$IMAGE" /opt/bin/lflare --psk "$PSK" --dev eth0 --forward "$port":6443
}

wait_session() {
  local name=$1
  local tries=${2:-30}
  for i in $(seq 1 "$tries"); do
    if docker logs "$name" 2>&1 | grep -c "session .* established" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

echo "== start client A (network A) and client B (network B) =="
start_client "$CLI_A" "$NET_A" 2222
start_client "$CLI_B" "$NET_B" 2223

if ! wait_session "$CLI_A"; then
  echo "FAIL: client A never established a session"
  docker logs "$CLI_A" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
if ! wait_session "$CLI_B"; then
  echo "FAIL: client B never established a session"
  docker logs "$CLI_B" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
if [ "$(docker logs "$SRV" 2>&1 | grep -c "authenticated, session")" -lt 2 ]; then
  echo "FAIL: server did not authenticate two clients"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "both clients authenticated"

transfer() {
  local name=$1 port=$2 tag=$3
  echo "== $tag =="
  local out
  out=$(docker exec "$name" bash -c '
    dd if=/dev/urandom of=/tmp/in.bin bs=1M count=2 status=none || exit 1
    md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
    nc -w 60 127.0.0.1 '"$port"' < /tmp/in.bin > /tmp/out.bin
    md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
    if cmp -s /tmp/in.md5 /tmp/out.md5; then echo OK; else echo "MISMATCH: $(cat /tmp/in.md5) vs $(cat /tmp/out.md5)"; fi
  ')
  echo "$out"
  [[ "$out" == OK ]]
}

echo "== concurrent transfers through both tunnels =="
transfer "$CLI_A" 2222 "client A transfer" &
pid_a=$!
transfer "$CLI_B" 2223 "client B transfer" &
pid_b=$!
wait "$pid_a" || { echo "FAIL: client A concurrent transfer"; docker logs "$SRV" 2>&1 | tail -20; exit 1; }
wait "$pid_b" || { echo "FAIL: client B concurrent transfer"; docker logs "$SRV" 2>&1 | tail -20; exit 1; }
echo "concurrent transfers OK"

echo "== teardown isolation: kill client A, client B must be unaffected =="
docker kill -s SIGTERM "$CLI_A" >/dev/null
for i in $(seq 1 10); do
  if docker logs "$SRV" 2>&1 | grep -c "sent teardown" >/dev/null; then
    break
  fi
  sleep 1
done
if ! docker logs "$SRV" 2>&1 | grep -c "sent teardown" >/dev/null; then
  echo "FAIL: server never saw client A's teardown"
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
transfer "$CLI_B" 2223 "client B transfer after A teardown"
echo "teardown isolation OK"

echo "== idle session outlives the stale timeout (keepalive echoes) =="
# 50s idle is longer than the server's 45s stale sweep; keepalives must
# keep the peer alive so this transfer still works.
sleep 50
transfer "$CLI_B" 2223 "client B transfer after 50s idle"
echo "idle keepalive OK"

echo "ALL MULTI-CLIENT E2E TESTS PASSED"
