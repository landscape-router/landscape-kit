#!/usr/bin/env bash
set -euo pipefail

# TODO(daemon-rewrite):`lkit service-manager` 命令已在移除 none 部署模式的
# 破坏性重构中删除,本脚本的迁移场景不再可执行。多后端重写(daemon 委托、
# OpenRC/sysvinit)已完成,本脚本应改为预置 systemd 已提交状态并部署
# `lkit self-service install` 的常驻 daemon,再以 `lkit switch`/`lkit uninstall`
# 触发真实 systemd 契约验证(注册、启停、MainPID、前端被杀后 daemon 子进程组
# 独立提交、所有权冲突)。
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
  --include=systemd,systemd-sysv,dbus,bash,util-linux,procps,iproute2,ca-certificates,python3 \
  trixie "$rootfs"

install -D -m 0755 "$prebuilt_dir/lkit" "$rootfs/usr/local/bin/lkit"
install -D -m 0755 "$prebuilt_dir/landscape-webserver" \
  "$rootfs/usr/local/bin/landscape-webserver"

install_root=$rootfs/var/lib/lkit-nspawn/landscape
release=$install_root/releases/1.0.0
mkdir -p "$release/static" "$install_root/data" "$install_root/state"
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

webserver_sha=$(sha256sum "$release/landscape-webserver" | awk '{print $1}')
webserver_size=$(stat -c '%s' "$release/landscape-webserver")
python3 - "$install_root/state/install-state.json" "$webserver_sha" "$webserver_size" <<'PY'
import json
import sys

path, webserver_sha, webserver_size = sys.argv[1:]
root = "/var/lib/lkit-nspawn/landscape"
state = {
    "schema_version": 1,
    "layout_version": 1,
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
        "manager": "none",
        "registered": False,
        "enabled": False,
        "verified": False,
        "definition_path": None,
        "definition_sha256": None,
    },
    "last_transaction_id": None,
    "committed_at": "2026-08-02T00:00:00Z",
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(state, stream, indent=2)
    stream.write("\n")
PY
chmod 0600 "$install_root/state/install-state.json"

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
  systemd-run \
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

# none -> systemd 会启动真实临时 worker unit。进入 verifying 后杀掉前端
# machinectl 会话，模拟 SSH 会话随 Landscape 重启而断开。
frontend_input=$test_root/frontend.input
mkfifo "$frontend_input"
exec 9<>"$frontend_input"
machinectl shell --quiet "root@$machine" /bin/bash -lc \
  "/usr/local/bin/lkit service-manager systemd --install-dir /var/lib/lkit-nspawn/landscape --test-runtime /var/lib/lkit-nspawn/runtime.json" \
  <"$frontend_input" >"$test_root/frontend.log" 2>&1 &
frontend_pid=$!

prompt_visible=false
for _ in $(seq 1 100); do
  if grep -q "stop your Landscape instance" "$test_root/frontend.log"; then
    prompt_visible=true
    break
  fi
  kill -0 "$frontend_pid" 2>/dev/null || break
  sleep 0.2
done
if [[ $prompt_visible != true ]]; then
  cat "$test_root/frontend.log" >&2
  echo "delegated migration did not request confirmation" >&2
  exit 1
fi
printf 'yes\n' >&9

reached_verifying=false
for _ in $(seq 1 100); do
  if machine_shell \
    "grep -q '\"phase\": \"verifying\"' /var/lib/lkit-nspawn/landscape/transactions/*.json 2>/dev/null"; then
    reached_verifying=true
    break
  fi
  sleep 0.2
done
if [[ $reached_verifying != true ]]; then
  cat "$test_root/frontend.log" >&2
  machine_shell \
    "ls -la /run/lkit/operations; cat /run/lkit/operations/*.log 2>/dev/null" >&2 || true
  machine_shell \
    "systemctl --no-pager --full status 'lkit-operation-*'" >&2 || true
  cat "$test_root/nspawn.log" >&2
  echo "delegated migration did not reach verifying" >&2
  exit 1
fi
kill "$frontend_pid" 2>/dev/null || true
wait "$frontend_pid" 2>/dev/null || true
exec 9>&-

committed=false
for _ in $(seq 1 100); do
  if machine_shell \
    "python3 -c 'import json; print(json.load(open(\"/var/lib/lkit-nspawn/landscape/state/install-state.json\"))[\"service\"][\"manager\"])' | grep -qx systemd"; then
    committed=true
    break
  fi
  sleep 0.2
done
if [[ $committed != true ]]; then
  cat "$test_root/frontend.log" >&2
  cat "$test_root/nspawn.log" >&2
  echo "worker did not commit after the frontend session was killed" >&2
  exit 1
fi

machine_shell "systemctl is-enabled --quiet landscape-router.service"
machine_shell "systemctl is-active --quiet landscape-router.service"
machine_shell "test \"\$(systemctl show --property=MainPID --value landscape-router.service)\" -gt 1"
worker_cleaned=false
for _ in $(seq 1 50); do
  if machine_shell "! systemctl list-units --all 'lkit-operation-*' --no-legend | grep -q ."; then
    worker_cleaned=true
    break
  fi
  sleep 0.2
done
[[ $worker_cleaned == true ]] || {
  echo "delegated worker unit was not cleaned after commit" >&2
  exit 1
}

# systemd -> none 也由 worker 执行，并必须停止、禁用和注销真实 unit。
machine_shell \
  "/usr/local/bin/lkit service-manager none --install-dir /var/lib/lkit-nspawn/landscape --test-runtime /var/lib/lkit-nspawn/runtime.json"
machine_shell \
  "python3 -c 'import json; print(json.load(open(\"/var/lib/lkit-nspawn/landscape/state/install-state.json\"))[\"service\"][\"manager\"])' | grep -qx none"
machine_shell "! systemctl is-active --quiet landscape-router.service"
machine_shell "test ! -e /etc/systemd/system/landscape-router.service"

# 所有权冲突必须在接管前失败,不留失败状态。
machine_shell "printf '[Unit]\nDescription=foreign unit\n' >/etc/systemd/system/landscape-router.service"
if machine_shell \
  "printf 'yes\\n' | script -qec '/usr/local/bin/lkit service-manager systemd --install-dir /var/lib/lkit-nspawn/landscape --test-runtime /var/lib/lkit-nspawn/runtime.json' /dev/null"; then
  conflict_status=0
else
  conflict_status=$?
fi
[[ $conflict_status -ne 0 ]] || {
  echo "systemd ownership conflict unexpectedly succeeded" >&2
  exit 1
}
machine_shell "rm /etc/systemd/system/landscape-router.service"
machine_shell \
  "python3 -c 'import json; print(json.load(open(\"/var/lib/lkit-nspawn/landscape/state/install-state.json\"))[\"service\"][\"manager\"])' | grep -qx none"
machine_shell \
  "! find /var/lib/lkit-nspawn/landscape/transactions -name '*.json' -exec grep -lE '\"phase\": \"(preparing|prepared|stopping|activating|verifying|rolling_back)\"' {} + | grep -q ."
conflict_worker_cleaned=false
for _ in $(seq 1 50); do
  if machine_shell "! systemctl list-units --all 'lkit-operation-*' --no-legend | grep -q ."; then
    conflict_worker_cleaned=true
    break
  fi
  sleep 0.2
done
[[ $conflict_worker_cleaned == true ]] || {
  echo "failed delegated worker unit was not cleaned" >&2
  exit 1
}

echo "PASS: systemd-nspawn worker and real-systemd integration"
