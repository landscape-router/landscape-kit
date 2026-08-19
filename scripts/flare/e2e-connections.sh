#!/usr/bin/env bash
# Connection-lifecycle end-to-end test for one client and one mapped port.
#
# Steps:
#   1. multiplex 32 simultaneous TCP connections through one mapping
#   2. serve concurrent HTTP/1.1 requests over persistent connections
#   3. recycle 480 clean short connections in concurrent waves
#   4. recycle 192 reset connections and verify fresh connections recover
#   5. keep accepting traffic while slow readers apply TCP backpressure
#   6. keep one established TCP connection alive across 50 seconds of idle
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=flare-connections
IMAGE=lndp-test:latest
SRV=landscape-connections-srv
CLI=landscape-connections-cli
PSK=secret
TOKEN=flare-connections-token

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

echo "== start server and client =="
docker run -d --name "$SRV" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c 'python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any --token '"$TOKEN"' --forward-ports 6443,8080'
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" \
    --forward 2222:6443 --forward 2280:8080

for i in $(seq 1 30); do
  if docker logs "$CLI" 2>&1 | grep -c "session .* established" >/dev/null; then
    break
  fi
  sleep 1
done
if ! docker logs "$CLI" 2>&1 | grep -c "session .* established" >/dev/null; then
  echo "FAIL: client never established a session"
  docker logs "$CLI" 2>&1 | tail -30
  docker logs "$SRV" 2>&1 | tail -30
  exit 1
fi

run_probe() {
  local label=$1 port=$2
  shift 2
  echo "== $label =="
  if ! docker exec "$CLI" python3 /opt/connection_probe.py "$@" --port "$port"; then
    echo "FAIL: $label"
    docker logs "$CLI" 2>&1 | tail -50
    docker logs "$SRV" 2>&1 | tail -50
    exit 1
  fi
}

run_probe "same-mapping concurrent connections" 2222 \
  concurrent --connections 32 --bytes 65536
run_probe "HTTP keep-alive concurrency" 2280 \
  http --connections 16 --requests 8
run_probe "clean short-connection churn" 2222 \
  churn --waves 20 --parallel 24 --bytes 2048
run_probe "reset cleanup and recovery" 2222 \
  resets --waves 8 --parallel 24 --bytes 2048
run_probe "slow-reader backpressure isolation" 2222 \
  backpressure --connections 12 --bytes 262144 --hold-seconds 2
run_probe "established connection survives idle period" 2222 \
  idle --seconds 50 --bytes 4096

echo "ALL CONNECTION-LIFECYCLE E2E TESTS PASSED"
