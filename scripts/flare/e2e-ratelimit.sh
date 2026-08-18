#!/usr/bin/env bash
# Rate-limit / lockout end-to-end test.
#
# The server protects itself with two per-MAC mechanisms:
#   - a token bucket (10 DISCOVER/AUTH_REQ per second) against scanning,
#     brute force and kick attempts;
#   - an auth lockout: 5 failed AUTH_REQs within 60s freeze that source
#     for 60s.
#
# Steps:
#   1. flood DISCOVER frames from a fake MAC; the server must rate limit
#      them and the fake MAC must never authenticate
#   2. a real client with a wrong psk must be rejected until it trips the
#      lockout, after which its attempts are ignored
#   3. a legitimate client is unaffected and can still transfer data
#
# Usage: test/e2e-ratelimit.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

NET=lndp-ratelimit
IMAGE=lndp-test:latest
SRV=landscape-rl-srv
FLOOD=landscape-rl-flood
CLI_BAD=landscape-rl-bad
CLI=landscape-rl-good
PSK=secret

cleanup() {
  docker rm -f "$SRV" "$FLOOD" "$CLI_BAD" "$CLI" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

echo "== build =="
cargo build --workspace >/dev/null
docker build -q -t "$IMAGE" -f scripts/flare/Dockerfile . >/dev/null
docker network create "$NET" >/dev/null

echo "== start server =="
docker run -d --name "$SRV" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" bash -c 'python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk '"$PSK"' --dev any'
sleep 1

echo "== flood DISCOVER from a fake MAC =="
docker run -d --name "$FLOOD" --network "$NET" \
  "$IMAGE" python3 /opt/rate_flood.py 60 3
docker wait "$FLOOD" >/dev/null
docker logs "$FLOOD"
sent=$(docker logs "$FLOOD" | grep -oE "sent [0-9]+" | grep -oE "[0-9]+")
if [ -z "$sent" ] || [ "$sent" -lt 100 ]; then
  echo "FAIL: flood sent too few frames ($sent)"
  exit 1
fi
sleep 1

limited=$(docker logs "$SRV" 2>&1 | grep -c "rate-limited" || true)
if [ "$limited" -lt 50 ]; then
  echo "FAIL: server rate-limited only $limited frames (expected >= 50)"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
echo "server rate-limited $limited flood frames"
if docker logs "$SRV" 2>&1 | grep -c "02:00:00:00:00:99.*authenticated" >/dev/null; then
  echo "FAIL: the flood MAC authenticated"
  exit 1
fi
echo "flood MAC never authenticated"
docker rm -f "$FLOOD" >/dev/null 2>&1 || true

echo "== wrong psk: server must stay silent (sealed DISCOVER) =="
# The DISCOVER is sealed with the psk, so a wrong-psk client is not even
# heard: the server never responds, never allocates a peer, and the client
# can never reach the auth stage. The lockout mechanism itself is covered
# by unit tests (only psk-holders can now trigger auth failures).
docker run -d --name "$CLI_BAD" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk wrong-secret --dev eth0 --forward 2325:6443
sleep 10
if docker logs "$CLI_BAD" 2>&1 | grep -c "session .* established" >/dev/null; then
  echo "FAIL: wrong-psk client established a session"
  exit 1
fi
if ! docker logs "$SRV" 2>&1 | grep -c "cannot open" >/dev/null; then
  echo "FAIL: server did not log the sealed-DISCOVER rejection"
  docker logs "$SRV" 2>&1 | tail -10
  exit 1
fi
echo "wrong psk stays unauthenticated (server silent)"
docker rm -f "$CLI_BAD" >/dev/null 2>&1 || true

echo "== legitimate client is unaffected =="
docker run -d --name "$CLI" --network "$NET" \
  --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
  --cap-add NET_RAW \
  -v "$PWD/target/debug:/opt/bin:ro" \
  "$IMAGE" /opt/bin/lflare cli --psk "$PSK" --dev eth0 --forward 2222:6443
established=""
for i in $(seq 1 30); do
  if docker logs "$CLI" 2>&1 | grep -c "session .* established" >/dev/null; then
    established=1
    break
  fi
  sleep 1
done
if [ -z "$established" ]; then
  echo "FAIL: legitimate client never established a session"
  docker logs "$CLI" 2>&1 | tail -20
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
out=$(docker exec "$CLI" bash -c '
  dd if=/dev/urandom of=/tmp/in.bin bs=1M count=1 status=none || exit 1
  md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
  nc -w 60 127.0.0.1 2222 < /tmp/in.bin > /tmp/out.bin
  md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
  if cmp -s /tmp/in.md5 /tmp/out.md5; then echo OK; else echo "MISMATCH"; fi
')
echo "transfer: $out"
[[ "$out" == OK ]]

echo "== spoofed auth failures must not lock out an active session =="
# 12 garbage AUTH_REQs sent with the legit client's own MAC (from its own
# container, so the bridge MAC learning stays intact). They cannot be opened
# with the handshake keys, so the server treats them as unauthentic frames:
# rejected without lockout accounting, never freezing the victim's
# re-authentication.
mac=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.MacAddress}}{{end}}' "$CLI")
if [ -z "$mac" ]; then
  echo "FAIL: could not read the client's MAC"
  exit 1
fi
docker exec "$CLI" python3 /opt/auth_req_flood.py 12
sleep 2
rejected=$(docker logs "$SRV" 2>&1 | grep -c "unauthentic auth frame" || true)
if [ "$rejected" -lt 5 ]; then
  echo "FAIL: expected >= 5 unauthentic auth frames from the spoof, saw $rejected"
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
# hard-kill the victim (no teardown): it must still reconnect immediately
# even though its MAC accumulated 12 rejected auth attempts
docker kill -s KILL "$CLI" >/dev/null
docker start "$CLI" >/dev/null
reconnected=""
for i in $(seq 1 60); do
  if [ "$(docker logs "$CLI" 2>&1 | grep -c "session .* established" || true)" -ge 2 ]; then
    reconnected=1
    break
  fi
  sleep 1
done
if [ -z "$reconnected" ]; then
  echo "FAIL: victim could not reconnect after spoofed failures (lockout?)"
  docker logs "$CLI" 2>&1 | tail -10
  docker logs "$SRV" 2>&1 | tail -20
  exit 1
fi
if docker logs "$SRV" 2>&1 | grep "ignored (lockout)" | grep -c "$mac" >/dev/null; then
  echo "FAIL: spoofed failures locked out MAC $mac"
  exit 1
fi
out=$(docker exec "$CLI" bash -c '
  dd if=/dev/urandom of=/tmp/in.bin bs=1M count=1 status=none || exit 1
  md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
  nc -w 60 127.0.0.1 2222 < /tmp/in.bin > /tmp/out.bin
  md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
  if cmp -s /tmp/in.md5 /tmp/out.md5; then echo OK; else echo "MISMATCH"; fi
')
echo "transfer after spoofed failures: $out"
[[ "$out" == OK ]]
echo "spoofed failures did not lock out the active session"

echo "ALL RATE-LIMIT E2E TESTS PASSED"
