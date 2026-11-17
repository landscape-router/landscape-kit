#!/usr/bin/env bash
# Remote shell payloads deliberately expand variables inside the guest, not here.
# shellcheck disable=SC2016
set -euo pipefail

# 真实 systemd 契约验证:在 systemd-nspawn 中部署 lkit 常驻 daemon
# (`lkit self install`),验证真实 manager 的注册、启停、MainPID 与
# `KillMode=process` 契约；原生操作验证覆盖提交、断开、运行中拒绝取消、
# daemon 崩溃恢复、不可用时拒绝、二进制替换与持久化操作语言。
#
# 每条 machine_shell 命令都有超时(LKIT_NSPAWN_CMD_TIMEOUT,默认 120s),卡住
# 的步骤会在约 2 分钟内失败并输出诊断,而不是空等到 workflow 超时。
# LKIT_NSPAWN_DEBUG=1 时打印每一步的命令与步骤标记,便于定位卡点。
LKIT_NSPAWN_DEBUG=${LKIT_NSPAWN_DEBUG:-0}
LKIT_NSPAWN_CMD_TIMEOUT=${LKIT_NSPAWN_CMD_TIMEOUT:-120}
if [[ $(uname -s):$(uname -m) != Linux:x86_64 ]]; then
  echo "systemd-nspawn integration currently requires Linux x86_64" >&2
  exit 2
fi
if [[ $EUID -ne 0 ]]; then
  echo "systemd-nspawn integration must run as root" >&2
  exit 2
fi
required_commands=(mmdebstrap systemd-nspawn machinectl systemd-run findmnt)
if [[ -z ${LKIT_NSPAWN_PREBUILT_DIR:-} ]]; then
  required_commands+=(cargo)
fi
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "$command_name is required for the systemd-nspawn integration" >&2
    exit 2
  }
done

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d /var/tmp/lkit-nspawn.XXXXXX)
rootfs=$test_root/rootfs
machine=lkit-nspawn-$$
machine_started=false
nspawn_pid=

# shellcheck source=scripts/lib/nspawn-cleanup.sh
source "$repo_root/scripts/lib/nspawn-cleanup.sh"
cleanup() {
  local status=$?
  trap - EXIT
  local result=0
  nspawn_cleanup_status "$status" "$machine_started" "$machine" "$nspawn_pid" "$test_root" || result=$?
  exit "$result"
}
trap cleanup EXIT

keyring_deb=$test_root/debian-archive-keyring.deb
keyring_root=$test_root/debian-keyring
curl --fail --location --silent --show-error \
  https://deb.debian.org/debian/pool/main/d/debian-archive-keyring/debian-archive-keyring_2025.1_all.deb \
  --output "$keyring_deb"
printf '%s  %s\n' \
  9ea7778e443144ca490668737a8ab22dd3e748bb99e805e22ec055abeb3c7fac \
  "$keyring_deb" | sha256sum --check --status
dpkg-deb --extract "$keyring_deb" "$keyring_root"
archive_keyring=$keyring_root/usr/share/keyrings/debian-archive-keyring.gpg
[[ -f $archive_keyring ]] || {
  echo "verified Debian archive keyring package did not contain its keyring" >&2
  exit 1
}

cd "$repo_root"
if [[ -n ${LKIT_NSPAWN_PREBUILT_DIR:-} ]]; then
  prebuilt_dir=$(realpath "$LKIT_NSPAWN_PREBUILT_DIR")
else
  nspawn_target=$test_root/target
  CARGO_TARGET_DIR="$nspawn_target" cargo build --locked --release --features test-support \
    -p lkit-cli --bin lkit
  CARGO_TARGET_DIR="$nspawn_target" cargo build --locked --release \
    -p lkit-test-fixture --bin landscape-webserver
  prebuilt_dir=$nspawn_target/release
fi
for binary in lkit landscape-webserver; do
  [[ -x $prebuilt_dir/$binary ]] || {
    echo "missing prebuilt executable $prebuilt_dir/$binary" >&2
    exit 2
  }
done

mmdebstrap \
  --variant=minbase \
  --keyring="$archive_keyring" \
  --include=systemd,systemd-sysv,dbus,bash,util-linux,procps,iproute2,ca-certificates,python3,zstd \
  trixie "$rootfs"

install -D -m 0644 "$repo_root/scripts/test-nspawn-networkd.py" "$rootfs/var/lib/lkit-nspawn/networkd-test.py"

install -D -m 0755 "$prebuilt_dir/lkit" "$rootfs/usr/local/bin/lkit"
install -D -m 0755 "$prebuilt_dir/landscape-webserver" \
  "$rootfs/usr/local/bin/landscape-webserver"

install_root=$rootfs/var/lib/lkit-nspawn/landscape
release=$install_root/releases/1.0.0
mkdir -p "$release/static" "$install_root/data" "$install_root/service" "$rootfs/root/.lkit/state"
install -m 0755 "$prebuilt_dir/landscape-webserver" "$release/landscape-webserver"
ln -s releases/1.0.0 "$install_root/current"
python3 - "$release/static.zip" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w") as archive:
    archive.writestr("static/index.html", "<h1>nspawn fixture</h1>")
PY
# live 的 static/ 必须与真实安装一致:保护备份从 current/static 现场打包
# static.zip 并自校验,顶层缺 index.html 会让备份失败(内容与 static.zip 相同)。
printf '<h1>nspawn fixture</h1>' >"$release/static/index.html"
cat >"$release/static/lkit-fixture.json" <<'JSON'
{
  "schema_version": 1,
  "scenario": "healthy",
  "listen_address": "127.0.0.1",
  "dns_tcp_port": 53,
  "dns_udp_port": 53,
  "http_port": 6300,
  "https_port": 6443,
  "ready_delay_ms": 750,
  "exit_after_ms": 30000,
  "start_exit_code": 1,
  "export_version": "1.0.0",
  "export_content": "version = \"1.0.0\"\n"
}
JSON
printf 'version = "1.0.0"\nadmin_user = "admin"\nadmin_pass = "Secret123"\n' \
  >"$install_root/data/landscape_init.toml"
chmod 0600 "$install_root/data/landscape_init.toml"
printf 'tok-1234567890abcdef\n' >"$install_root/data/landscape_api_token"
chmod 0400 "$install_root/data/landscape_api_token"

# 预置受管 unit 原件与系统注册链接(state 与真实 systemd 一致,内容与
# render_unit 完全一致;注册与启动在 machine 起来后由真实 systemd 完成)。
cat >"$install_root/service/landscape-router.service" <<'UNIT'
[Unit]
Description=Landscape Router
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/var/lib/lkit-nspawn/landscape/current/landscape-webserver --config-dir /var/lib/lkit-nspawn/landscape/data --web /var/lib/lkit-nspawn/landscape/current/static
User=root
Restart=always
LimitMEMLOCK=infinity

[Install]
WantedBy=multi-user.target
UNIT
chmod 0600 "$install_root/service/landscape-router.service"
ln -s /var/lib/lkit-nspawn/landscape/service/landscape-router.service \
  "$rootfs/etc/systemd/system/landscape-router.service"

webserver_sha=$(sha256sum "$release/landscape-webserver" | awk '{print $1}')
webserver_size=$(stat -c '%s' "$release/landscape-webserver")
static_sha=$(sha256sum "$release/static.zip" | awk '{print $1}')
static_size=$(stat -c '%s' "$release/static.zip")
unit_sha=$(sha256sum "$install_root/service/landscape-router.service" | awk '{print $1}')
python3 - "$rootfs/root/.lkit/state/install-state.json" "$webserver_sha" "$webserver_size" "$static_sha" "$static_size" "$unit_sha" <<'PY'
import json
import sys

path, webserver_sha, webserver_size, static_sha, static_size, unit_sha = sys.argv[1:]
root = "/var/lib/lkit-nspawn/landscape"
state = {
    "schema_version": 1,
    "layout_version": 2,
    "install_root": root,
    "canonical_install_root": root,
    "active_version": "1.0.0",
    "repository": {"kind": "github", "location": "ThisSeanZhang/landscape"},
    "assets": {
        "webserver": {
            "architecture": "x86_64",
            "sha256": webserver_sha,
            "size": int(webserver_size),
        },
        "static_archive": {"sha256": static_sha, "size": int(static_size)},
    },
    "initialization": {
        "status": "pending",
        "lock_present": False,
        "initialized_at": None,
    },
    "service": {
        "manager": "systemd",
        "registered": True,
        "enabled": True,
        "verified": True,
        "definition_path": "/var/lib/lkit-nspawn/landscape/service/landscape-router.service",
        "definition_sha256": unit_sha,
    },
    "last_transaction_id": None,
    "committed_at": "2026-08-02T00:00:00Z",
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(state, stream, indent=2)
    stream.write("\n")
PY
chmod 0600 "$rootfs/root/.lkit/state/install-state.json"

cat >"$rootfs/var/lib/lkit-nspawn/runtime.json" <<'JSON'
{
  "schema_version": 1,
  "allow_non_root": false,
  "preflight": "skip",
  "execution": "daemon",
  "managed_uid": 0,
  "os_release_path": "/etc/os-release",
  "systemd": {
    "systemctl": "/bin/systemctl",
    "system_unit_dir": "/etc/systemd/system",
    "run_systemd_dir": "/run/systemd/system",
    "pid1_is_systemd": true,
    "resolv_conf": "/etc/resolv.conf"
  },
  "health": {
    "base_url": "https://127.0.0.1:6443",
    "dns_tcp_port": 53,
    "dns_udp_port": 53,
    "http_port": 6300,
    "https_port": 6443,
    "startup_timeout_ms": 10000,
    "stable_duration_ms": 8000
  },
  "export_base_url": "https://127.0.0.1:6443"
}
JSON

systemd-nspawn \
  --quiet \
  --boot \
  --directory "$rootfs" \
  --machine "$machine" \
  --private-network \
  >"$test_root/nspawn.log" 2>&1 &
nspawn_pid=$!
machine_started=true

echo "== boot the nspawn machine"
for ((i = 0; i < 100; i++)); do
  machinectl show "$machine" >/dev/null 2>&1 && break
  kill -0 "$nspawn_pid" 2>/dev/null || {
    cat "$test_root/nspawn.log" >&2
    exit 1
  }
  sleep 0.2
done
machinectl show "$machine" >/dev/null 2>&1 || {
  cat "$test_root/nspawn.log" >&2
  echo "nspawn machine did not become ready" >&2
  exit 1
}

# machine_shell / machine_shell_bg 的 payload 会经 systemd-run 变成瞬态单元
# 的 ExecStart:systemd 按单元(空)环境展开 `${VAR...}`(含 `${VAR#pat}`、
# `${VAR%pat}`,未定义即空串),但不触碰 `$(...)` 与裸 `$VAR`。payload 内
# 严禁使用花括号参数展开——需要截断用 cut,剥后缀用 basename 的 suffix 参数。
machine_shell() {
  if [[ $LKIT_NSPAWN_DEBUG == 1 ]]; then
    echo ">> machine: $1" >&2
  fi
  timeout "$LKIT_NSPAWN_CMD_TIMEOUT" systemd-run \
    --machine "$machine" \
    --wait \
    --pipe \
    --collect \
    --quiet \
    /bin/bash -lc "$1"
}

# 与 machine_shell 相同,但 scope 使用 `KillMode=process`:命令退出后后台
# 子进程继续存活(模拟脱离 systemd-run 会话的前端 CLI 进程,与 lkit.service
# 的 KillMode 语义一致),供前端断开/取消场景使用。
machine_shell_bg() {
  if [[ $LKIT_NSPAWN_DEBUG == 1 ]]; then
    echo ">> machine(bg): $1" >&2
  fi
  set +e
  timeout "$LKIT_NSPAWN_CMD_TIMEOUT" systemd-run \
    --machine "$machine" \
    --wait \
    --pipe \
    --collect \
    --property=KillMode=process \
    /bin/bash -lc "$1"
  bg_status=$?
  set -e
  echo "machine_shell_bg exit: $bg_status" >&2
  return "$bg_status"
}

machine_dump() {
  local label="$1"
  echo "===== MACHINE DUMP: $label ====="
  machine_shell 'echo "--- observer output:"; cat /tmp/s3.out /tmp/s3.err 2>/dev/null; echo "--- operation records:"; cat /root/.lkit/run/operations/*.json 2>/dev/null' || true
  machine_shell 'echo "--- transactions:"; for f in /root/.lkit/transactions/*.json; do [ -e "$f" ] || continue; echo "== $f"; cat "$f"; echo; done' || true
  machine_shell 'echo "--- state:"; cat /root/.lkit/state/install-state.json 2>/dev/null || echo "absent"' || true
  machine_shell 'echo "--- operations:"; ls -la /root/.lkit/run/operations/ 2>/dev/null || echo "none"' || true
  machine_shell 'echo "--- lkit.service:"; systemctl is-active lkit.service; journalctl -u lkit.service -n 40 --no-pager 2>/dev/null | tail -40' || true
  machine_shell 'echo "--- landscape-router:"; systemctl is-active landscape-router.service' || true
  machine_shell 'echo "--- run dir:"; ls -la /root/.lkit/run/ 2>/dev/null || echo "none"' || true
  echo "===== END MACHINE DUMP ====="
}

system_bus_ready=false
for ((i = 0; i < 100; i++)); do
  if machine_shell "true" >/dev/null 2>&1; then
    system_bus_ready=true
    break
  fi
  kill -0 "$nspawn_pid" 2>/dev/null || break
  sleep 0.2
done
if [[ $system_bus_ready != true ]]; then
  cat "$test_root/nspawn.log" >&2
  echo "nspawn system bus did not become ready" >&2
  exit 1
fi

# 真实 systemd 契约:注册并启动受管的 landscape-router 服务。
echo "== register and start landscape-router.service"
machine_shell "systemctl daemon-reload"
machine_shell "systemctl enable --quiet landscape-router.service"
machine_shell "systemctl start landscape-router.service"
machine_shell "systemctl is-enabled --quiet landscape-router.service"
machine_shell "systemctl is-active --quiet landscape-router.service"
machine_shell "test \"\$(systemctl show --property=MainPID --value landscape-router.service)\" -gt 1"

# 部署常驻 daemon:self install 在真实 systemd 下注册并启动 lkit.service。
echo "== deploy the resident daemon via self install"
machine_shell \
  "/usr/local/bin/lkit self install --test-runtime /var/lib/lkit-nspawn/runtime.json"
echo "   self: enabled"
machine_shell "systemctl is-enabled --quiet lkit.service"
echo "   self: active"
machine_shell "systemctl is-active --quiet lkit.service"
echo "   self: mainpid"
machine_shell "test \"\$(systemctl show --property=MainPID --value lkit.service)\" -gt 1"
echo "   self: killmode"
machine_shell "systemctl show --property=KillMode --value lkit.service | grep -qx process"
echo "   self: pidfile"
machine_shell "test -f /root/.lkit/run/lkit.pid"

echo "== daemon crash restarts without replacing the supervisor"
machine_shell '
  supervisor=$(systemctl show --property=MainPID --value lkit.service)
  previous=$(cat /root/.lkit/run/lkit.pid)
  kill -KILL "$previous"
  for attempt in $(seq 1 30); do
    current=$(cat /root/.lkit/run/lkit.pid 2>/dev/null || true)
    if [ -n "$current" ] && [ "$current" != "$previous" ] && kill -0 "$current" 2>/dev/null; then
      test "$(systemctl show --property=MainPID --value lkit.service)" = "$supervisor"
      exit 0
    fi
    sleep 1
  done
  exit 1
'

# 真实 systemd 契约:受管服务可停止并重新启动(不涉及卸载/注销)。
echo "== stop and restart the managed services"
echo "   restart: stop landscape"
machine_shell "systemctl stop landscape-router.service"
echo "   restart: landscape inactive"
machine_shell "! systemctl is-active --quiet landscape-router.service"
echo "   restart: start landscape"
machine_shell "systemctl start landscape-router.service"
echo "   restart: landscape active"
machine_shell "systemctl is-active --quiet landscape-router.service"
echo "   restart: stop lkit"
machine_shell "systemctl stop lkit.service"
echo "   restart: lkit inactive"
machine_shell "! systemctl is-active --quiet lkit.service"
echo "   restart: start lkit"
machine_shell "systemctl start lkit.service"
echo "   restart: lkit active"
machine_shell "systemctl is-active --quiet lkit.service"
echo "   restart: pidfile"
machine_shell "test -f /root/.lkit/run/lkit.pid"

# ============ 原生 daemon 操作兼容性 ============

restore_scene() {
  machine_shell 'rm -f /root/.lkit/transactions/*.json 2>/dev/null; rm -f /root/.lkit/run/operations/*.json 2>/dev/null; true'
  mkdir -p "$install_root/releases/1.0.0/static" "$install_root/data" "$install_root/service"
  install -m 0755 "$prebuilt_dir/landscape-webserver" \
    "$install_root/releases/1.0.0/landscape-webserver"
  python3 - "$install_root/releases/1.0.0/static.zip" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w") as archive:
    archive.writestr("static/index.html", "<h1>nspawn fixture</h1>")
PY
  ln -sfn releases/1.0.0 "$install_root/current"
  # 与初始拼装一致:live static/ 顶层要有 index.html,否则保护备份打包失败。
  printf '<h1>nspawn fixture</h1>' >"$install_root/releases/1.0.0/static/index.html"
  cat >"$install_root/releases/1.0.0/static/lkit-fixture.json" <<'JSON'
{
  "schema_version": 1,
  "scenario": "healthy",
  "listen_address": "127.0.0.1",
  "dns_tcp_port": 53,
  "dns_udp_port": 53,
  "http_port": 6300,
  "https_port": 6443,
  "ready_delay_ms": 750,
  "exit_after_ms": 30000,
  "start_exit_code": 1,
  "export_version": "1.0.0",
  "export_content": "version = \"1.0.0\"\n"
}
JSON
  printf 'version = "1.0.0"\nadmin_user = "admin"\nadmin_pass = "Secret123"\n' \
    >"$install_root/data/landscape_init.toml"
  chmod 0600 "$install_root/data/landscape_init.toml"
  printf 'tok-1234567890abcdef\n' >"$install_root/data/landscape_api_token"
  chmod 0400 "$install_root/data/landscape_api_token"
  cat >"$install_root/service/landscape-router.service" <<'UNIT'
[Unit]
Description=Landscape Router
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/var/lib/lkit-nspawn/landscape/current/landscape-webserver --config-dir /var/lib/lkit-nspawn/landscape/data --web /var/lib/lkit-nspawn/landscape/current/static
User=root
Restart=always
LimitMEMLOCK=infinity

[Install]
WantedBy=multi-user.target
UNIT
  chmod 0600 "$install_root/service/landscape-router.service"
  ln -sfn /var/lib/lkit-nspawn/landscape/service/landscape-router.service \
    "$rootfs/etc/systemd/system/landscape-router.service"
  python3 - "$rootfs/root/.lkit/state/install-state.json" "$webserver_sha" "$webserver_size" "$static_sha" "$static_size" "$unit_sha" <<'PY'
import json
import sys

path, webserver_sha, webserver_size, static_sha, static_size, unit_sha = sys.argv[1:]
root = "/var/lib/lkit-nspawn/landscape"
state = {
    "schema_version": 1,
    "layout_version": 2,
    "install_root": root,
    "canonical_install_root": root,
    "active_version": "1.0.0",
    "repository": {"kind": "github", "location": "ThisSeanZhang/landscape"},
    "assets": {
        "webserver": {
            "architecture": "x86_64",
            "sha256": webserver_sha,
            "size": int(webserver_size),
        },
        "static_archive": {"sha256": static_sha, "size": int(static_size)},
    },
    "initialization": {
        "status": "pending",
        "lock_present": False,
        "initialized_at": None,
    },
    "service": {
        "manager": "systemd",
        "registered": True,
        "enabled": True,
        "verified": True,
        "definition_path": "/var/lib/lkit-nspawn/landscape/service/landscape-router.service",
        "definition_sha256": unit_sha,
    },
    "last_transaction_id": None,
    "committed_at": "2026-08-02T00:00:00Z",
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(state, stream, indent=2)
    stream.write("\n")
PY
  chmod 0600 "$rootfs/root/.lkit/state/install-state.json"
  echo "   restore: daemon-reload"
  machine_shell "systemctl daemon-reload"
  echo "   restore: enable"
  machine_shell "systemctl enable --quiet landscape-router.service"
  echo "   restore: start"
  machine_shell "systemctl start landscape-router.service"
  echo "   restore: wait-active"
  machine_shell 'for ((i = 0; i < 100; i++)); do systemctl is-active --quiet landscape-router.service && break; sleep 0.1; done; systemctl is-active --quiet landscape-router.service'
  echo "   restore: wait-ready"
  machine_shell 'for ((i = 0; i < 200; i++)); do (exec 3<>/dev/tcp/127.0.0.1/6443) 2>/dev/null && break; sleep 0.1; done; (exec 3<>/dev/tcp/127.0.0.1/6443) 2>/dev/null || exit 1'
}

# Native operation receipts are the observable contract; the smoke never looks
# for a child lkit process or a transient worker output file.
cat >"$rootfs/var/lib/lkit-nspawn/operation.py" <<'PY'
import glob
import json
import sys

records = [json.load(open(path, encoding="utf-8")) for path in
           glob.glob("/root/.lkit/run/operations/*.json")]
records = sorted((r for r in records if r["kind"] == "uninstall"), key=lambda r: r["id"])
if not records:
    sys.exit(1)
record = records[-1]
if sys.argv[1] == "id":
    print(record["id"])
elif sys.argv[1] == "status":
    sys.exit(0 if record["status"] == sys.argv[2] else 1)
elif sys.argv[1] == "language":
    assert record["language"] == "zh", record
    assert "卸载" in record["message"], record
else:
    raise ValueError(sys.argv[1])
PY

wait_receipt() {
  machine_shell "for ((i = 0; i < 300; i++)); do python3 /var/lib/lkit-nspawn/operation.py status '$1' && exit 0; sleep 0.2; done; exit 1"
}

checkpoint_on() {
  machine_shell 'mkdir -p /etc/systemd/system/lkit.service.d /root/.lkit/run
printf "[Service]\nEnvironment=LKIT_TEST_REPAIR_CHECKPOINT=uninstall-stopping\n" >/etc/systemd/system/lkit.service.d/checkpoint.conf
rm -f /root/.lkit/run/test-repair-uninstall-stopping.reached
touch /root/.lkit/run/test-repair-uninstall-stopping.gate
systemctl daemon-reload
systemctl restart lkit.service
for ((i = 0; i < 100; i++)); do
  P=$(cat /root/.lkit/run/lkit.pid 2>/dev/null || true)
  C=$(cat /root/.lkit/run/uninstall-v1.pid 2>/dev/null || true)
  if [ -n "$P" ] && [ "$P" = "$C" ] && kill -0 "$P" 2>/dev/null; then exit 0; fi
  sleep 0.1
done
exit 1'
}
checkpoint_off() {
  machine_shell 'rm -f /root/.lkit/run/test-repair-uninstall-stopping.gate /etc/systemd/system/lkit.service.d/checkpoint.conf
systemctl daemon-reload'
}
start_observer() {
  machine_shell_bg 'nohup /usr/local/bin/lkit --non-interactive uninstall --yes --test-runtime /var/lib/lkit-nspawn/runtime.json >/tmp/observer.out 2>/tmp/observer.err </dev/null & echo $! >/tmp/observer.pid'
  machine_shell 'for ((i = 0; i < 300; i++)); do test -f /root/.lkit/run/test-repair-uninstall-stopping.reached && exit 0; sleep 0.1; done; cat /tmp/observer.err; exit 1'
}

echo "== native S-1: submit and query the committed uninstall"
machine_shell \
  "/usr/local/bin/lkit --non-interactive uninstall --yes --test-runtime /var/lib/lkit-nspawn/runtime.json"
wait_receipt succeeded
machine_shell 'OPID=$(python3 /var/lib/lkit-nspawn/operation.py id); /usr/local/bin/lkit operation status "$OPID"'
machine_shell "test ! -f /root/.lkit/state/install-state.json"
machine_shell "test ! -L /etc/systemd/system/landscape-router.service"
machine_shell "test ! -e /var/lib/lkit-nspawn/landscape/current"
machine_shell "! systemctl is-active --quiet landscape-router.service"
machine_shell "systemctl is-active --quiet lkit.service"
machine_shell "test \"\$(find /root/.lkit/backups -name '*.lkb' | wc -l)\" -ge 1"

echo "== native S-2: frontend disconnect does not cancel the operation"
restore_scene
checkpoint_on
start_observer
machine_shell 'kill -9 "$(cat /tmp/observer.pid)"'
checkpoint_off
wait_receipt succeeded
machine_shell "test ! -f /root/.lkit/state/install-state.json"
machine_shell "systemctl is-active --quiet lkit.service"

echo "== native S-3: running cancellation is refused; daemon crash recovers forward"
restore_scene
checkpoint_on
start_observer
machine_shell 'OPID=$(python3 /var/lib/lkit-nspawn/operation.py id)
echo "$OPID" >/tmp/recovery-operation
if /usr/local/bin/lkit operation cancel "$OPID"; then exit 1; fi
python3 /var/lib/lkit-nspawn/operation.py status running
kill -KILL "$(cat /root/.lkit/run/lkit.pid)"'
checkpoint_off
wait_receipt succeeded
machine_shell 'test "$(python3 /var/lib/lkit-nspawn/operation.py id)" = "$(cat /tmp/recovery-operation)"'
machine_shell "test ! -f /root/.lkit/state/install-state.json"
machine_shell "! systemctl is-active --quiet landscape-router.service"
machine_shell "systemctl is-active --quiet lkit.service"

echo "== native S-4: unavailable daemon rejects submission promptly"
restore_scene
machine_shell "systemctl stop lkit.service"
machine_shell 'timeout 10 /usr/local/bin/lkit --non-interactive uninstall --yes --test-runtime /var/lib/lkit-nspawn/runtime.json >/tmp/s4.out 2>/tmp/s4.err
CODE=$?
test "$CODE" -ne 0 && test "$CODE" -ne 124'
machine_shell 'test -f /root/.lkit/state/install-state.json; test -z "$(find /root/.lkit/run/operations -name "*.json" -print)"'
machine_shell "systemctl start lkit.service"

echo "== native S-4b: executable replacement does not block native execution"
machine_shell 'for ((i = 0; i < 100; i++)); do
  P=$(cat /root/.lkit/run/lkit.pid 2>/dev/null || true)
  C=$(cat /root/.lkit/run/uninstall-v1.pid 2>/dev/null || true)
  if [ -n "$P" ] && [ "$P" = "$C" ] && kill -0 "$P" 2>/dev/null; then break; fi
  sleep 0.1
done
cp /usr/local/bin/lkit /tmp/lkit-observer
rm /usr/local/bin/lkit
/tmp/lkit-observer --non-interactive uninstall --yes --test-runtime /var/lib/lkit-nspawn/runtime.json
RESULT=$?
install -m 0755 /tmp/lkit-observer /usr/local/bin/lkit
rm /tmp/lkit-observer
test "$RESULT" -eq 0 && test "$(cat /root/.lkit/run/lkit.pid)" = "$P"'
wait_receipt succeeded

echo "== native S-5: the operation carries its selected language"
restore_scene
machine_shell 'LKIT_LANG=zh /usr/local/bin/lkit --non-interactive uninstall --yes --test-runtime /var/lib/lkit-nspawn/runtime.json >/tmp/s5.out 2>/tmp/s5.err'
wait_receipt succeeded
machine_shell 'python3 /var/lib/lkit-nspawn/operation.py language'

echo "== native N-1: real networkd takeover and rollback"
machine_shell "python3 /var/lib/lkit-nspawn/networkd-test.py"

echo "PASS: systemd-nspawn registration, lifecycle, networkd and native daemon operations"
