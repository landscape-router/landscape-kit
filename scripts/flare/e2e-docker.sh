#!/usr/bin/env bash
# Docker-based end-to-end test.
#
# Topology: two containers on one L2 bridge network, i.e. two hosts on the
# same ethernet segment:
#
#   client container (lflare cli --dev eth0 --forward 2222:6443)
#        |  LNDP frames (ethertype 0x88B6, broadcast + unicast)
#        v
#   server container (lndp-server --dev any, fake service on 127.0.0.1:6443)
#
# Steps:
#   1. handshake + base data integrity (2 MiB through the tunnel)
#   2. inject 10% packet loss on the server's eth0 (tc netem)
#   3. repeat the transfer, expect identical md5 (smoltcp retransmits)
#   4. whitelist rejection: forward to a non-allowed port must close
#   5. anti-scanning: client without token must not get a session
#   6. wrong psk: client with a bad secret must be rejected (mutual auth)
#   7. teardown: SIGTERM the client; the server must drop it immediately
#   8. replay injection: stale DATA frames must be dropped, transfer intact
#   9. server restart: client must reconnect and the tunnel must work again
#
# Usage: test/e2e-docker.sh [--loss-only]
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=lndp-test
IMAGE=lndp-test:latest
SRV=landscape-srv
CLI=landscape-cli
CLI2=landscape-cli-whitelist
CLI3=landscape-cli-notoken
CLI4=landscape-cli-badpsk
PSK=secret
TOKEN=lndp-token
LOSS_PERCENT=10

cleanup() {
  docker rm -f "$SRV" "$CLI" "$CLI2" "$CLI3" "$CLI4" >/dev/null 2>&1 || true
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
  --cap-add NET_RAW --cap-add NET_ADMIN \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c 'python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any --token '"$TOKEN"

echo "== start client =="
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward 2222:6443

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

echo "== wait for session =="
if ! wait_session "$CLI"; then
  echo "FAIL: client never established a session"
  docker logs "$CLI" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi

transfer() {
  local tag=$1
  echo "== $tag =="
  local out
  out=$(docker exec "$CLI" bash -c '
    dd if=/dev/urandom of=/tmp/in.bin bs=1M count=2 status=none || exit 1
    md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
    nc -w 60 127.0.0.1 2222 < /tmp/in.bin > /tmp/out.bin
    md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
    if cmp -s /tmp/in.md5 /tmp/out.md5; then echo OK; else echo "MISMATCH: $(cat /tmp/in.md5) vs $(cat /tmp/out.md5)"; fi
  ')
  echo "$out"
  [[ "$out" == OK ]]
}

transfer "base transfer (no loss)"

if [[ "${1:-}" == "--loss-only" ]]; then
  exit 0
fi

echo "== inject ${LOSS_PERCENT}% loss on server eth0 =="
docker exec "$SRV" tc qdisc add dev eth0 root netem loss ${LOSS_PERCENT}%
transfer "transfer with ${LOSS_PERCENT}% packet loss"
docker exec "$SRV" tc qdisc del dev eth0 root

echo "== whitelist: forward to 8080 must be rejected =="
docker run -d --name "$CLI2" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward 2323:8080
sleep 4
# the smoltcp leg always establishes transiently; what matters is that the
# server closes the connection after reading the forbidden target port
if docker exec "$CLI2" bash -c '
  exec 3<>/dev/tcp/127.0.0.1/2323 || { echo "connect failed"; exit 1; }
  timeout 3 cat <&3 >/dev/null
  if [ $? -eq 0 ]; then echo "rejected OK (server closed)"; else echo "FAIL: connection stayed open"; exit 1; fi
'; then
  echo "whitelist rejection OK"
else
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
docker rm -f "$CLI2" >/dev/null 2>&1 || true

echo "== anti-scanning: client without token must not get a session =="
docker run -d --name "$CLI3" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --forward 2324:6443
sleep 8
if docker logs "$CLI3" 2>&1 | grep -c "session .* established" >/dev/null; then
  echo "FAIL: tokenless client established a session"
  exit 1
fi
echo "tokenless client stays unauthenticated OK"
if ! docker logs "$SRV" 2>&1 | grep -c "token mismatch" >/dev/null; then
  echo "FAIL: server did not log the token mismatch"
  exit 1
fi
docker rm -f "$CLI3" >/dev/null 2>&1 || true

echo "== wrong psk: client with a bad secret must be rejected =="
docker run -d --name "$CLI4" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk wrong-secret --dev eth0 --token "$TOKEN" --forward 2325:6443
sleep 8
if docker logs "$CLI4" 2>&1 | grep -c "session .* established" >/dev/null; then
  echo "FAIL: client with wrong psk established a session"
  exit 1
fi
# The DISCOVER is sealed with the psk, so the server must stay silent:
# the client is rejected at discovery, never reaching the auth stage.
if ! docker logs "$SRV" 2>&1 | grep -c "cannot open" >/dev/null; then
  echo "FAIL: server did not reject the wrong-psk DISCOVER"
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
echo "wrong psk rejected OK"
docker rm -f "$CLI4" >/dev/null 2>&1 || true

echo "== teardown: SIGTERM the client, server must drop it immediately =="
docker kill -s SIGTERM "$CLI" >/dev/null
for i in $(seq 1 10); do
  if docker logs "$SRV" 2>&1 | grep -c "sent teardown" >/dev/null; then
    break
  fi
  sleep 1
done
if ! docker logs "$SRV" 2>&1 | grep -c "sent teardown" >/dev/null; then
  echo "FAIL: server never saw the client teardown"
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
echo "teardown OK"
docker rm -f "$CLI" >/dev/null 2>&1 || true
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --token "$TOKEN" --forward 2222:6443
if ! wait_session "$CLI"; then
  echo "FAIL: client did not re-establish after the teardown test"
  exit 1
fi

echo "== replay injection: stale DATA frames must be dropped =="
docker exec -d "$CLI" bash -c 'python3 /opt/replay_inject.py 8 > /tmp/replay.log 2>&1'
transfer "transfer with replayed frames injected"
for i in $(seq 1 10); do
  if docker exec "$CLI" cat /tmp/replay.log 2>/dev/null | grep -c "captured" >/dev/null; then
    break
  fi
  sleep 1
done
if ! docker exec "$CLI" cat /tmp/replay.log 2>/dev/null | grep -Eq "captured [1-9].*injected [1-9]"; then
  echo "FAIL: replayer did not capture and inject any frames"
  docker exec "$CLI" cat /tmp/replay.log 2>/dev/null || true
  exit 1
fi
docker exec "$CLI" cat /tmp/replay.log 2>/dev/null
echo "replay injection OK"

echo "== server restart: client must reconnect and transfer again =="
docker restart "$SRV" >/dev/null
before=$(docker logs "$CLI" 2>&1 | grep -c "session .* established")
for i in $(seq 1 45); do
  now=$(docker logs "$CLI" 2>&1 | grep -c "session .* established")
  if [ "$now" -gt "$before" ]; then
    break
  fi
  sleep 1
done
if [ "$(docker logs "$CLI" 2>&1 | grep -c 'session .* established')" -le "$before" ]; then
  echo "FAIL: client never reconnected after server restart"
  docker logs "$CLI" 2>&1 | tail -10
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
echo "client reconnected after server restart"
transfer "transfer after server restart"

echo "ALL E2E TESTS PASSED"
