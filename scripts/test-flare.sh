#!/usr/bin/env bash
# Landscape Terrain (flare) 协议 e2e 测试：L2 bridge 双容器拓扑。
#
# 拓扑：同一 L2 网段(或双网段)上的两个容器：
#
#   client 容器 (lflare --dev eth0 --forward 2222:6443)
#        |  Terrain 帧 (ethertype 0x88B6, broadcast + unicast)
#        v
#   server 容器 (lkit flare serve --dev any, fake service on 127.0.0.1:6443)
#
# 场景：
#   single-segment  握手/传输/丢包/白名单/令牌/错误 psk/teardown/重放/重启
#   same-segment    同段多客户端、并发传输、优雅重启与硬杀恢复
#   multiclient     双网段多客户端、teardown 隔离、空闲保活
#   ratelimit       限速桶、锁死防护、伪造 auth 失败不冻结活动会话
#
# 用法：scripts/test-flare.sh
set -euo pipefail

case $(uname -s):$(uname -m) in
  Linux:x86_64) ;;
  Linux:aarch64|Linux:arm64)
    if [[ ${LKIT_E2E_ALLOW_ARM:-} != 1 ]]; then
      echo "flare E2E is supported locally only on Linux x86_64; use CI for aarch64" >&2
      exit 2
    fi
    ;;
  *)
    echo "flare E2E requires Linux" >&2
    exit 2
    ;;
esac

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for the flare E2E" >&2
  exit 1
fi

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

echo "== build =="
cargo build --workspace >/dev/null
scripts/docker/flare/fetch-packages.sh >/dev/null
IMAGE=flare-e2e:latest
docker build -q -t "$IMAGE" -f scripts/docker/flare/Dockerfile scripts/docker/flare >/dev/null

# 场景公共函数：容器二进制从宿主 target/debug 挂载,lflare 与 lkit 都是
# workspace 产物;scrypt 用最小成本因子加速测试(双方一致即可)。
start_server() {
  local net=$1 name=$2 psk=$3 token=$4
  local cmd="python3 /opt/fake_service.py & exec /opt/bin/lkit flare serve --psk $psk --dev any"
  if [[ -n $token ]]; then
    cmd="$cmd --token $token"
  fi
  docker run -d --name "$name" --network "$net" \
    --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
    --cap-add NET_RAW --cap-add NET_ADMIN \
    -v "$root/target/debug:/opt/bin:ro" \
    "$IMAGE" bash -c "$cmd"
}

start_client() {
  local net=$1 name=$2 psk=$3 token=$4 forward=$5
  shift 5
  local args=()
  if [[ -n $token ]]; then
    args+=(--token "$token")
  fi
  docker run -d --name "$name" --network "$net" \
    --env LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10 \
    --cap-add NET_RAW \
    -v "$root/target/debug:/opt/bin:ro" \
    "$IMAGE" /opt/bin/lflare --psk "$psk" --dev eth0 "${args[@]}" --forward "$forward" "$@"
}

wait_session() {
  local name=$1
  local tries=${2:-30}
  for _ in $(seq 1 "$tries"); do
    if docker logs "$name" 2>&1 | grep -q "session .* established"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_sessions() {
  local name=$1 want=$2
  local tries=${3:-60}
  for _ in $(seq 1 "$tries"); do
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
  echo "== $tag =="
  local out
  out=$(docker exec "$name" bash -c '
    dd if=/dev/urandom of=/tmp/in.bin bs=1M count='"$size"' status=none || exit 1
    md5sum /tmp/in.bin | cut -d" " -f1 > /tmp/in.md5
    nc -w 60 127.0.0.1 '"$port"' < /tmp/in.bin > /tmp/out.bin
    md5sum /tmp/out.bin | cut -d" " -f1 > /tmp/out.md5
    if cmp -s /tmp/in.md5 /tmp/out.md5; then echo OK; else echo "MISMATCH: $(cat /tmp/in.md5) vs $(cat /tmp/out.md5)"; fi
  ')
  echo "$out"
  [[ "$out" == OK ]]
}

# 场景 1：单网段全功能(single-segment)
single_segment() {
  local net=flare-e2e-seg
  local srv=flare-e2e-srv cli=flare-e2e-cli cli2=flare-e2e-cli-whitelist
  local cli3=flare-e2e-cli-notoken cli4=flare-e2e-cli-badpsk
  local psk=secret token=flare-token loss=10

  cleanup() {
    docker rm -f "$srv" "$cli" "$cli2" "$cli3" "$cli4" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  cleanup

  docker network create "$net" >/dev/null
  start_server "$net" "$srv" "$psk" "$token"

  echo "== single-segment: start client =="
  start_client "$net" "$cli" "$psk" "$token" 2222:6443
  if ! wait_session "$cli"; then
    echo "FAIL: client never established a session"
    docker logs "$cli" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi

  transfer "$cli" 2222 2 "base transfer (no loss)"

  echo "== inject ${loss}% loss on server eth0 =="
  docker exec "$srv" tc qdisc add dev eth0 root netem loss ${loss}%
  transfer "$cli" 2222 2 "transfer with ${loss}% packet loss"
  docker exec "$srv" tc qdisc del dev eth0 root

  echo "== whitelist: forward to 8080 must be rejected =="
  start_client "$net" "$cli2" "$psk" "$token" 2323:8080
  sleep 4
  # the smoltcp leg always establishes transiently; what matters is that the
  # server closes the connection after reading the forbidden target port
  if ! docker exec "$cli2" bash -c '
    exec 3<>/dev/tcp/127.0.0.1/2323 || { echo "connect failed"; exit 1; }
    timeout 3 cat <&3 >/dev/null
    if [ $? -eq 0 ]; then echo "rejected OK (server closed)"; else echo "FAIL: connection stayed open"; exit 1; fi
  '; then
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  echo "whitelist rejection OK"
  docker rm -f "$cli2" >/dev/null 2>&1 || true

  echo "== anti-scanning: client without token must not get a session =="
  start_client "$net" "$cli3" "$psk" "" 2324:6443
  sleep 8
  if docker logs "$cli3" 2>&1 | grep -q "session .* established"; then
    echo "FAIL: tokenless client established a session"
    exit 1
  fi
  if ! docker logs "$srv" 2>&1 | grep -q "token mismatch"; then
    echo "FAIL: server did not log the token mismatch"
    exit 1
  fi
  echo "tokenless client stays unauthenticated OK"
  docker rm -f "$cli3" >/dev/null 2>&1 || true

  echo "== wrong psk: client with a bad secret must be rejected =="
  start_client "$net" "$cli4" wrong-secret "$token" 2325:6443
  sleep 8
  if docker logs "$cli4" 2>&1 | grep -q "session .* established"; then
    echo "FAIL: client with wrong psk established a session"
    exit 1
  fi
  # The DISCOVER is sealed with the psk, so the server must stay silent:
  # the client is rejected at discovery, never reaching the auth stage.
  if ! docker logs "$srv" 2>&1 | grep -q "cannot open"; then
    echo "FAIL: server did not reject the wrong-psk DISCOVER"
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  echo "wrong psk rejected OK"
  docker rm -f "$cli4" >/dev/null 2>&1 || true

  echo "== teardown: SIGTERM the client, server must drop it immediately =="
  docker kill -s SIGTERM "$cli" >/dev/null
  for _ in $(seq 1 10); do
    if docker logs "$srv" 2>&1 | grep -q "sent teardown"; then
      break
    fi
    sleep 1
  done
  if ! docker logs "$srv" 2>&1 | grep -q "sent teardown"; then
    echo "FAIL: server never saw the client teardown"
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  echo "teardown OK"
  docker rm -f "$cli" >/dev/null 2>&1 || true
  start_client "$net" "$cli" "$psk" "$token" 2222:6443
  if ! wait_session "$cli"; then
    echo "FAIL: client did not re-establish after the teardown test"
    exit 1
  fi

  echo "== replay injection: stale DATA frames must be dropped =="
  docker exec -d "$cli" bash -c 'python3 /opt/replay_inject.py 8 > /tmp/replay.log 2>&1'
  transfer "$cli" 2222 2 "transfer with replayed frames injected"
  for _ in $(seq 1 10); do
    if docker exec "$cli" cat /tmp/replay.log 2>/dev/null | grep -q "captured"; then
      break
    fi
    sleep 1
  done
  if ! docker exec "$cli" cat /tmp/replay.log 2>/dev/null | grep -Eq "captured [1-9].*injected [1-9]"; then
    echo "FAIL: replayer did not capture and inject any frames"
    docker exec "$cli" cat /tmp/replay.log 2>/dev/null || true
    exit 1
  fi
  docker exec "$cli" cat /tmp/replay.log 2>/dev/null
  echo "replay injection OK"

  echo "== server restart: client must reconnect and transfer again =="
  docker restart "$srv" >/dev/null
  local before
  before=$(docker logs "$cli" 2>&1 | grep -c "session .* established" || true)
  for _ in $(seq 1 45); do
    local now
    now=$(docker logs "$cli" 2>&1 | grep -c "session .* established" || true)
    if [ "$now" -gt "$before" ]; then
      break
    fi
    sleep 1
  done
  if [ "$(docker logs "$cli" 2>&1 | grep -c 'session .* established' || true)" -le "$before" ]; then
    echo "FAIL: client never reconnected after server restart"
    docker logs "$cli" 2>&1 | tail -10
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  echo "client reconnected after server restart"
  transfer "$cli" 2222 2 "transfer after server restart"

  echo "ALL SINGLE-SEGMENT E2E TESTS PASSED"
}

# 场景 2：同段多客户端(same-segment)
same_segment() {
  local net=flare-e2e-seg2
  local srv=flare-e2e-seg2-srv cli_a=flare-e2e-seg2-a cli_b=flare-e2e-seg2-b
  local psk=secret token=flare-token

  cleanup() {
    docker rm -f "$srv" "$cli_a" "$cli_b" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  cleanup

  docker network create "$net" >/dev/null
  start_server "$net" "$srv" "$psk" "$token"

  echo "== same-segment: start clients A and B =="
  start_client "$net" "$cli_a" "$psk" "$token" 2222:6443
  start_client "$net" "$cli_b" "$psk" "$token" 2223:6443
  if ! wait_sessions "$cli_a" 1; then
    echo "FAIL: client A never connected"
    docker logs "$cli_a" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  if ! wait_sessions "$cli_b" 1; then
    echo "FAIL: client B never connected"
    docker logs "$cli_b" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  if [ "$(docker logs "$srv" 2>&1 | grep -c "authenticated, session" || true)" -lt 2 ]; then
    echo "FAIL: server did not authenticate two clients"
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  echo "both clients authenticated on the same segment"

  echo "== concurrent transfers =="
  transfer "$cli_a" 2222 2 "client A transfer" &
  local pid_a=$!
  transfer "$cli_b" 2223 2 "client B transfer" &
  local pid_b=$!
  wait "$pid_a" || { echo "FAIL: client A concurrent transfer"; docker logs "$srv" 2>&1 | tail -20; exit 1; }
  wait "$pid_b" || { echo "FAIL: client B concurrent transfer"; docker logs "$srv" 2>&1 | tail -20; exit 1; }
  echo "concurrent transfers OK"

  echo "== graceful restart of A (SIGTERM teardown + reconnect, same MAC) =="
  docker restart "$cli_a" >/dev/null
  if ! wait_sessions "$cli_a" 2; then
    echo "FAIL: client A did not reconnect after restart"
    docker logs "$cli_a" 2>&1 | tail -10
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  transfer "$cli_a" 2222 2 "client A transfer after restart"
  echo "graceful restart OK"

  echo "== hard kill of B (SIGKILL, stale session must be replaced) =="
  docker kill -s KILL "$cli_b" >/dev/null
  docker start "$cli_b" >/dev/null
  if ! wait_sessions "$cli_b" 2 60; then
    echo "FAIL: client B did not recover after SIGKILL"
    docker logs "$cli_b" 2>&1 | tail -10
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  transfer "$cli_b" 2223 2 "client B transfer after SIGKILL recovery"
  echo "hard-kill session replacement OK"

  echo "== larger sustained transfer (20 MiB) =="
  transfer "$cli_a" 2222 20 "client A 20MiB transfer"

  echo "ALL SAME-SEGMENT E2E TESTS PASSED"
}

# 场景 3：双网段多客户端(multiclient)
multiclient() {
  local net_a=flare-e2e-mc-a net_b=flare-e2e-mc-b
  local srv=flare-e2e-mc-srv cli_a=flare-e2e-mc-a cli_b=flare-e2e-mc-b
  local psk=secret

  cleanup() {
    docker rm -f "$srv" "$cli_a" "$cli_b" >/dev/null 2>&1 || true
    docker network rm "$net_a" "$net_b" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  cleanup

  docker network create "$net_a" >/dev/null
  docker network create "$net_b" >/dev/null

  echo "== multiclient: start server on both networks =="
  start_server "$net_a" "$srv" "$psk" ""
  docker network connect "$net_b" "$srv"
  sleep 1

  echo "== start client A (network A) and client B (network B) =="
  start_client "$net_a" "$cli_a" "$psk" "" 2222:6443
  start_client "$net_b" "$cli_b" "$psk" "" 2223:6443

  if ! wait_session "$cli_a"; then
    echo "FAIL: client A never established a session"
    docker logs "$cli_a" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  if ! wait_session "$cli_b"; then
    echo "FAIL: client B never established a session"
    docker logs "$cli_b" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  if [ "$(docker logs "$srv" 2>&1 | grep -c "authenticated, session")" -lt 2 ]; then
    echo "FAIL: server did not authenticate two clients"
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  echo "both clients authenticated"

  echo "== concurrent transfers through both tunnels =="
  transfer "$cli_a" 2222 2 "client A transfer" &
  local pid_a=$!
  transfer "$cli_b" 2223 2 "client B transfer" &
  local pid_b=$!
  wait "$pid_a" || { echo "FAIL: client A concurrent transfer"; docker logs "$srv" 2>&1 | tail -20; exit 1; }
  wait "$pid_b" || { echo "FAIL: client B concurrent transfer"; docker logs "$srv" 2>&1 | tail -20; exit 1; }
  echo "concurrent transfers OK"

  echo "== teardown isolation: kill client A, client B must be unaffected =="
  docker kill -s SIGTERM "$cli_a" >/dev/null
  for _ in $(seq 1 10); do
    if docker logs "$srv" 2>&1 | grep -q "sent teardown"; then
      break
    fi
    sleep 1
  done
  if ! docker logs "$srv" 2>&1 | grep -q "sent teardown"; then
    echo "FAIL: server never saw client A's teardown"
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  transfer "$cli_b" 2223 2 "client B transfer after A teardown"
  echo "teardown isolation OK"

  echo "== idle session outlives the stale timeout (keepalive echoes) =="
  # 50s idle is longer than the server's 45s stale sweep; keepalives must
  # keep the peer alive so this transfer still works.
  sleep 50
  transfer "$cli_b" 2223 2 "client B transfer after 50s idle"
  echo "idle keepalive OK"

  echo "ALL MULTI-CLIENT E2E TESTS PASSED"
}

# 场景 4：限速与锁死(ratelimit)
ratelimit() {
  local net=flare-e2e-rl
  local srv=flare-e2e-rl-srv flood=flare-e2e-rl-flood
  local cli_bad=flare-e2e-rl-bad cli=flare-e2e-rl-good
  local psk=secret

  cleanup() {
    docker rm -f "$srv" "$flood" "$cli_bad" "$cli" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  cleanup

  docker network create "$net" >/dev/null
  start_server "$net" "$srv" "$psk" ""
  sleep 1

  echo "== flood DISCOVER from a fake MAC =="
  docker run -d --name "$flood" --network "$net" \
    "$IMAGE" python3 /opt/rate_flood.py 60 3
  docker wait "$flood" >/dev/null
  docker logs "$flood"
  local sent
  sent=$(docker logs "$flood" | grep -oE "sent [0-9]+" | grep -oE "[0-9]+" || true)
  if [ -z "$sent" ] || [ "$sent" -lt 100 ]; then
    echo "FAIL: flood sent too few frames ($sent)"
    exit 1
  fi
  sleep 1

  local limited
  limited=$(docker logs "$srv" 2>&1 | grep -c "rate-limited" || true)
  if [ "$limited" -lt 50 ]; then
    echo "FAIL: server rate-limited only $limited frames (expected >= 50)"
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  echo "server rate-limited $limited flood frames"
  if docker logs "$srv" 2>&1 | grep -q "02:00:00:00:00:99.*authenticated"; then
    echo "FAIL: the flood MAC authenticated"
    exit 1
  fi
  echo "flood MAC never authenticated"
  docker rm -f "$flood" >/dev/null 2>&1 || true

  echo "== wrong psk: server must stay silent (sealed DISCOVER) =="
  # The DISCOVER is sealed with the psk, so a wrong-psk client is not even
  # heard: the server never responds, never allocates a peer, and the client
  # can never reach the auth stage. The lockout mechanism itself is covered
  # by unit tests (only psk-holders can now trigger auth failures).
  start_client "$net" "$cli_bad" wrong-secret "" 2325:6443
  sleep 10
  if docker logs "$cli_bad" 2>&1 | grep -q "session .* established"; then
    echo "FAIL: wrong-psk client established a session"
    exit 1
  fi
  if ! docker logs "$srv" 2>&1 | grep -q "cannot open"; then
    echo "FAIL: server did not log the sealed-DISCOVER rejection"
    docker logs "$srv" 2>&1 | tail -10
    exit 1
  fi
  echo "wrong psk stays unauthenticated (server silent)"
  docker rm -f "$cli_bad" >/dev/null 2>&1 || true

  echo "== legitimate client is unaffected =="
  start_client "$net" "$cli" "$psk" "" 2222:6443
  if ! wait_session "$cli"; then
    echo "FAIL: legitimate client never established a session"
    docker logs "$cli" 2>&1 | tail -20
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  transfer "$cli" 2222 1 "transfer under flood"

  echo "== spoofed auth failures must not lock out an active session =="
  # 12 wrong-proof AUTH_REQs sent with the legit client's own MAC (from its
  # own container, so the bridge MAC learning stays intact). The server must
  # reject them but never freeze the victim's re-authentication.
  local mac
  mac=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.MacAddress}}{{end}}' "$cli")
  if [ -z "$mac" ]; then
    echo "FAIL: could not read the client's MAC"
    exit 1
  fi
  docker exec "$cli" python3 /opt/auth_req_flood.py 12
  sleep 2
  local rejected
  rejected=$(docker logs "$srv" 2>&1 | grep -c "auth rejected for" || true)
  if [ "$rejected" -lt 5 ]; then
    echo "FAIL: expected >= 5 auth rejections from the spoof, saw $rejected"
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  # hard-kill the victim (no teardown): it must still reconnect immediately
  # even though its MAC accumulated 12 rejected auth attempts
  docker kill -s KILL "$cli" >/dev/null
  docker start "$cli" >/dev/null
  if ! wait_sessions "$cli" 2 60; then
    echo "FAIL: victim could not reconnect after spoofed failures (lockout?)"
    docker logs "$cli" 2>&1 | tail -10
    docker logs "$srv" 2>&1 | tail -20
    exit 1
  fi
  if docker logs "$srv" 2>&1 | grep "ignored (lockout)" | grep -q "$mac"; then
    echo "FAIL: spoofed failures locked out MAC $mac"
    exit 1
  fi
  transfer "$cli" 2222 1 "transfer after spoofed failures"
  echo "spoofed failures did not lock out the active session"

  echo "ALL RATE-LIMIT E2E TESTS PASSED"
}

# 每个场景在子 shell 中运行,EXIT trap 只清理自己的容器与网络。
single_segment
same_segment
multiclient
ratelimit

echo "ALL FLARE E2E TESTS PASSED"
