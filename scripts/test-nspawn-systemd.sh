#!/usr/bin/env bash
set -euo pipefail

# 真实 systemd 契约验证:在 systemd-nspawn 中部署 lkit 常驻 daemon
# (`lkit self install`),验证真实 manager 的注册、启停、MainPID 与
# `KillMode=process` 契约。卸载场景当前不处理(见 docs/testing/nspawn-systemd.md),
# 由 OpenRC/sysvinit 后端 fixture E2E 覆盖的委托执行边界不在此重复。
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
required_commands=(mmdebstrap systemd-nspawn machinectl systemd-run)
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

cleanup() {
  if [[ $machine_started == true ]]; then
    machinectl terminate "$machine" >/dev/null 2>&1 || true
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT

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
  --include=systemd,systemd-sysv,dbus,bash,util-linux,procps,iproute2,ca-certificates,python3 \
  trixie "$rootfs"

install -D -m 0755 "$prebuilt_dir/lkit" "$rootfs/usr/local/bin/lkit"
install -D -m 0755 "$prebuilt_dir/landscape-webserver" \
  "$rootfs/usr/local/bin/landscape-webserver"

install_root=$rootfs/var/lib/lkit-nspawn/landscape
release=$install_root/releases/1.0.0
mkdir -p "$release/static" "$install_root/data" "$install_root/service" "$rootfs/root/.lkit/state"
install -m 0755 "$prebuilt_dir/landscape-webserver" "$release/landscape-webserver"
ln -s releases/1.0.0 "$install_root/current"
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
  "exit_after_ms": 2000,
  "start_exit_code": 1,
  "export_version": "1.0.0",
  "export_content": "version = \"1.0.0\"\n"
}
JSON
printf 'version = "1.0.0"\nadmin_user = "admin"\nadmin_pass = "Secret123"\n' \
  >"$install_root/data/landscape_init.toml"
chmod 0600 "$install_root/data/landscape_init.toml"

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
unit_sha=$(sha256sum "$install_root/service/landscape-router.service" | awk '{print $1}')
python3 - "$rootfs/root/.lkit/state/install-state.json" "$webserver_sha" "$webserver_size" "$unit_sha" <<'PY'
import json
import sys

path, webserver_sha, webserver_size, unit_sha = sys.argv[1:]
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
        "static_archive": {"sha256": "0" * 64, "size": 1},
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
for _ in $(seq 1 100); do
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

system_bus_ready=false
for _ in $(seq 1 100); do
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
machine_shell "systemctl is-enabled --quiet lkit.service"
machine_shell "systemctl is-active --quiet lkit.service"
machine_shell "test \"\$(systemctl show --property=MainPID --value lkit.service)\" -gt 1"
machine_shell "systemctl show --property=KillMode --value lkit.service | grep -qx process"
machine_shell "test -f /root/.lkit/run/lkit.pid"

# 真实 systemd 契约:受管服务可停止并重新启动(不涉及卸载/注销)。
echo "== stop and restart the managed services"
machine_shell "systemctl stop landscape-router.service"
machine_shell "! systemctl is-active --quiet landscape-router.service"
machine_shell "systemctl start landscape-router.service"
machine_shell "systemctl is-active --quiet landscape-router.service"
machine_shell "systemctl stop lkit.service"
machine_shell "! systemctl is-active --quiet lkit.service"
machine_shell "systemctl start lkit.service"
machine_shell "systemctl is-active --quiet lkit.service"
machine_shell "test -f /root/.lkit/run/lkit.pid"

# 当前节点不处理卸载:委托的 uninstall 需要交互确认,而 daemon 子进程没有
# 终端(`cannot open /dev/tty`),确认委托机制尚未实现。所有权冲突、前端断开后
# daemon 独立完成卸载等场景待文档明确需求后再补。
echo "PASS: systemd-nspawn real-systemd registration and lifecycle"
