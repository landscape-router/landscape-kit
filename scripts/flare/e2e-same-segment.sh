#!/usr/bin/env bash
# Same-segment multi-client end-to-end test.
#
# One server, one L2 segment (one docker bridge), several clients. This is
# the default LAN topology: sessions are keyed by the client MAC.
#
#   client A ─┐
#   client B ─┼── switch ── server
#
# Steps:
#   1. two clients on the same segment establish their own sessions
#   2. concurrent transfers through both tunnels are intact
#   3. graceful restart (SIGTERM): client A reconnects with the same MAC
#   4. hard kill (SIGKILL, no teardown): client B's stale session must be
#      replaced by a new handshake, transfer works again
#   5. a larger sustained transfer (20 MiB) completes intact
#
# Usage: test/e2e-same-segment.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=lndp-seg
IMAGE=lndp-test:latest
SRV=landscape-seg-srv
CLI_A=landscape-seg-a
CLI_B=landscape-seg-b
PSK=secret
TOKEN=lndp-token

cleanup() {
  docker rm -f "$SRV" "$CLI_A" "$CLI_B" >/dev/null 2>&1 || true
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

echo "== start server =="
docker run -d --name "$SRV" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c 'python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any --token '"$TOKEN"

start_client() {
  local name=$1 port=$2
  docker run -d --name "$name" --network "$NET" \
    --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
    --cap-add NET_RAW \
    -v "$PWD/target/debug:/opt/bin:ro" \
    "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward "$port":6443
}

# Wait until the client has logged at least `want` session-establishments.
# `docker logs` accumulates across restarts of the same container (same MAC).
wait_sessions() {
  local name=$1 want=$2
  local tries=${3:-60}
  for i in $(seq 1 "$tries"); do
    local now
    now=$(docker logs "$name" 2>&1 | grep -c "session .* established" || true)
    if [ "$now" -ge "$want" ]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

transfer() {
  local name=$1 port=$2 size=$3 tag=$4
  local wait_secs=${5:-60}
  echo "== $tag =="
  local out
  out=$(docker exec "$name" bash -c '
    dd if=/dev/urandom of=/tmp/in.bin bs=1M count='"$size"' status=none || exit 1
    md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
    nc -w '"$wait_secs"' 127.0.0.1 '"$port"' < /tmp/in.bin > /tmp/out.bin
    md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
    if cmp -s /tmp/in.md5 /tmp/out.md5; then
      echo "OK ($(wc -c < /tmp/out.bin) bytes)"
    else
      echo "MISMATCH: $(cat /tmp/in.md5) vs $(cat /tmp/out.md5); bytes=$(wc -c < /tmp/out.bin)/$(wc -c < /tmp/in.bin)"
    fi
  ')
  echo "$out"
  if [[ "$out" != OK* ]]; then
    echo "== client log tail ($name) =="
    docker logs "$name" 2>&1 | tail -30 || true
    echo "== server log tail ($SRV) =="
    docker logs "$SRV" 2>&1 | tail -40 || true
  fi
  [[ "$out" == OK* ]]
}

echo "== start clients A and B on the same segment =="
start_client "$CLI_A" 2222
start_client "$CLI_B" 2223
if ! wait_sessions "$CLI_A" 1; then
  echo "FAIL: client A never connected"
  docker logs "$CLI_A" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
if ! wait_sessions "$CLI_B" 1; then
  echo "FAIL: client B never connected"
  docker logs "$CLI_B" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
if [ "$(docker logs "$SRV" 2>&1 | grep -c "authenticated, session" || true)" -lt 2 ]; then
  echo "FAIL: server did not authenticate two clients"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "both clients authenticated on the same segment"

echo "== concurrent transfers =="
transfer "$CLI_A" 2222 2 "client A transfer" &
pid_a=$!
transfer "$CLI_B" 2223 2 "client B transfer" &
pid_b=$!
wait "$pid_a" || { echo "FAIL: client A concurrent transfer"; docker logs "$SRV" 2>&1 | tail -20; exit 1; }
wait "$pid_b" || { echo "FAIL: client B concurrent transfer"; docker logs "$SRV" 2>&1 | tail -20; exit 1; }
echo "concurrent transfers OK"

echo "== graceful restart of A (SIGTERM teardown + reconnect, same MAC) =="
docker restart "$CLI_A" >/dev/null
if ! wait_sessions "$CLI_A" 2; then
  echo "FAIL: client A did not reconnect after restart"
  docker logs "$CLI_A" 2>&1 | tail -10
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
transfer "$CLI_A" 2222 2 "client A transfer after restart"
echo "graceful restart OK"

echo "== hard kill of B (SIGKILL, stale session must be replaced) =="
docker kill -s KILL "$CLI_B" >/dev/null
docker start "$CLI_B" >/dev/null
if ! wait_sessions "$CLI_B" 2 60; then
  echo "FAIL: client B did not recover after SIGKILL"
  docker logs "$CLI_B" 2>&1 | tail -10
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
transfer "$CLI_B" 2223 2 "client B transfer after SIGKILL recovery"
echo "hard-kill session replacement OK"

echo "== larger sustained transfer (20 MiB) =="
transfer "$CLI_A" 2222 20 "client A 20MiB transfer" 600

echo "ALL SAME-SEGMENT E2E TESTS PASSED"
