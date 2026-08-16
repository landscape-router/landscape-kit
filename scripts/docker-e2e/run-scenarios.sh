#!/usr/bin/env bash
set -euo pipefail

# 功能 E2E 验证 lkit 与受支持服务管理器后端的协议。生产二进制不包含
# `--test-runtime`;这里使用 test-support 构建显式注入 fake systemctl。
# 多后端重构后 `lkit service-manager` 与 `--service-manager`/`none` 模式已删除:
# 场景顺序为 主安装根全生命周期 → S7 以 uninstall 释放注册链接与端口后安装
# latest 根 → S8 在 latest 根上验证中断事务恢复 → S2 再以 uninstall 释放后在
# export 根上验证导出失败回滚。
source /usr/local/lib/lkit-e2e/rustfs-test.sh

endpoint=${RUSTFS_ENDPOINT:-http://rustfs:9000}
bucket=${RUSTFS_BUCKET:-lkit-lifecycle}
public_base=${RUSTFS_PUBLIC_BASE_URL:-http://rustfs:9000/$bucket}
access_key=${AWS_ACCESS_KEY_ID:-lkit-test-access-key}
secret_key=${AWS_SECRET_ACCESS_KEY:-lkit-test-secret-key}
region=${AWS_REGION:-us-east-1}
install_root=/var/lib/lkit-e2e/landscape
install_root_export=/var/lib/lkit-e2e/landscape-export-error
install_root_latest=/var/lib/lkit-e2e/landscape-latest
territory_root=/root/.lkit
work_directory=/var/lib/lkit-e2e/work
password_file=/var/lib/lkit-e2e/password
host_root=/var/lib/lkit-e2e/host
runtime_config=$work_directory/runtime.json
systemctl_config=$work_directory/systemctl.json
fixture_resolv_conf=$host_root/resolv.conf

# 功能 E2E 只验证 lkit 与 systemd 服务管理器后端的协议。生产二进制不包含
# `--test-runtime`;这里使用 test-support 构建显式注入 fake systemctl。
export LKIT_TEST_SYSTEMCTL_CONFIG=$systemctl_config

lkit() {
  local subcommand=$1
  shift
  case $subcommand in
    install|switch|repair|reconcile|backup|restore|uninstall)
      command /usr/local/bin/lkit "$subcommand" "$@" --test-runtime "$runtime_config"
      ;;
    *) command /usr/local/bin/lkit "$subcommand" "$@" ;;
  esac
}

systemctl() {
  command /usr/local/bin/lkit-test-systemctl "$@"
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

signed_curl() {
  rustfs_signed_curl "$endpoint" "$access_key" "$secret_key" "$region" "$@"
}

public_get() {
  curl --fail-with-body --silent --show-error "$1"
}

json_value() {
  local path=$1
  local expression=$2
  python3 - "$path" "$expression" <<'PY'
import json
import sys

path, expression = sys.argv[1:]
with open(path, encoding="utf-8") as stream:
    value = json.load(stream)
for component in expression.split("."):
    value = value[component]
if isinstance(value, bool):
    print(str(value).lower())
else:
    print(value)
PY
}

assert_state_version() {
  local expected=$1
  local root=${2:-$install_root}
  local state=$territory_root/state/install-state.json
  local actual
  actual=$(json_value "$state" active_version)
  [[ $actual == "$expected" ]] || fail "active version expected $expected, got $actual"
  [[ $(readlink -f "$root/current") == "$root/releases/$expected" ]] \
    || fail "current does not point to releases/$expected"
}

assert_service_identity() {
  local root=${1:-$install_root}
  local state=$territory_root/state/install-state.json
  systemctl is-enabled --quiet landscape-router.service \
    || fail "landscape-router.service is not enabled"
  systemctl is-active --quiet landscape-router.service \
    || fail "landscape-router.service is not active"
  local pid expected_sha actual_sha executable
  pid=$(systemctl show --property=MainPID --value landscape-router.service)
  [[ $pid =~ ^[1-9][0-9]*$ ]] || fail "invalid MainPID: $pid"
  executable=$(readlink -f "/proc/$pid/exe")
  [[ $executable == "$root/releases/"*/landscape-webserver ]] \
    || fail "unexpected service executable $executable"
  expected_sha=$(json_value "$state" assets.webserver.sha256)
  actual_sha=$(sha256sum "$executable" | awk '{print $1}')
  [[ $actual_sha == "$expected_sha" ]] \
    || fail "service executable sha mismatch: expected $expected_sha, got $actual_sha"
  curl --fail --silent --show-error --insecure https://127.0.0.1:6443/api/docs >/dev/null \
    || fail "fixture HTTPS docs probe failed"
}

latest_transaction() {
  local root=${1:-$territory_root}
  find "$root/transactions" -maxdepth 1 -type f -name '*.json' -printf '%T@ %p\n' \
    | sort -n | tail -n1 | cut -d' ' -f2-
}

latest_backup() {
  local root=${1:-$territory_root}
  find "$root/backups" -maxdepth 1 -type f -name '*.lkb' -printf '%T@ %p\n' \
    | sort -n | tail -n1 | cut -d' ' -f2-
}

lkb_count() {
  local root=${1:-$territory_root}
  find "$root/backups" -maxdepth 1 -type f -name '*.lkb' 2>/dev/null | wc -l
}

assert_latest_phase() {
  local expected=$1
  local root=${2:-$territory_root}
  local transaction
  transaction=$(latest_transaction "$root")
  [[ -n "$transaction" ]] || fail "no transaction found under $root"
  local phase
  phase=$(json_value "$transaction" phase)
  [[ $phase == "$expected" ]] || fail "latest transaction phase expected $expected, got $phase"
}

assert_no_unfinished() {
  local root=$1
  local leftover
  leftover=$(python3 - "$root" <<'PY'
import json
import os
import sys

root = sys.argv[1]
tx_dir = os.path.join(root, "transactions")
if not os.path.isdir(tx_dir):
    sys.exit(0)
leftovers = []
for name in sorted(os.listdir(tx_dir)):
    if not name.endswith(".json"):
        continue
    with open(os.path.join(tx_dir, name), encoding="utf-8") as stream:
        tx = json.load(stream)
    if tx["phase"] not in ("committed", "rolled_back", "failed"):
        leftovers.append(f"{name}:{tx['phase']}")
if leftovers:
    print(",".join(leftovers))
PY
)
  [[ -z "$leftover" ]] || fail "unfinished transactions remain: $leftover"
}

assert_backup_metadata() {
  local backup=$1
  local expected_version=$2
  local expected_architecture=$3
  python3 - "$backup" "$expected_version" "$expected_architecture" <<'PY'
import gzip
import hashlib
import io
import json
import re
import sys
import tarfile

path, expected_version, expected_architecture = sys.argv[1:]
with open(path, "rb") as stream:
    content = stream.read()
assert len(content) > 1024 * 1024
assert content[:4] == b"LKB1"
assert int.from_bytes(content[4:6], "little") == 1
metadata_length = int.from_bytes(content[6:10], "little")
metadata = json.loads(content[32:32 + metadata_length])
assert metadata["landscape_version"] == expected_version
assert metadata["architecture"] == expected_architecture
assert metadata["auto"] is True
assert metadata["scope"] == "minimal"
assert re.fullmatch(r"\d{8}-\d{6}-[0-9a-f]{8}", metadata["backup_id"])
archive = content[1024 * 1024:]
expected = metadata["checksum"].removeprefix("sha256:")
assert hashlib.sha256(archive).hexdigest() == expected
with tarfile.open(fileobj=io.BytesIO(gzip.decompress(archive))) as tar:
    names = tar.getnames()
    assert "static.zip" in names, "backup must carry the static archive"
    assert "landscape-webserver" in names
    assert "landscape_init.toml" in names
    members = {member.name: member for member in tar.getmembers()}
    assert "static" in members and members["static"].isdir(), "backup must carry the static tree"
    assert "geo_tmp" in members and members["geo_tmp"].isdir(), "backup must carry the geo cache tree"
PY
}

assert_manual_backup_metadata() {
  local backup=$1
  local expected_version=$2
  local expected_architecture=$3
  local expected_remark=$4
  python3 - "$backup" "$expected_version" "$expected_architecture" "$expected_remark" <<'PY'
import gzip
import hashlib
import io
import json
import re
import sys
import tarfile

path, expected_version, expected_architecture, expected_remark = sys.argv[1:]
with open(path, "rb") as stream:
    content = stream.read()
assert content[:4] == b"LKB1"
metadata_length = int.from_bytes(content[6:10], "little")
metadata = json.loads(content[32:32 + metadata_length])
assert metadata["landscape_version"] == expected_version
assert metadata["architecture"] == expected_architecture
assert metadata["auto"] is False
assert metadata["remark"] == expected_remark
assert metadata["contents"]["static_archive"] is True
assert re.fullmatch(r"\d{8}-\d{6}-[0-9a-f]{8}", metadata["backup_id"])
archive = content[1024 * 1024:]
expected = metadata["checksum"].removeprefix("sha256:")
assert hashlib.sha256(archive).hexdigest() == expected
with tarfile.open(fileobj=io.BytesIO(gzip.decompress(archive))) as tar:
    assert "static.zip" in tar.getnames()
PY
}

publish_release() {
  local version=$1
  local scenario=$2
  local ready_delay=${3:-750}
  local release_directory=$work_directory/releases/$version
  mkdir -p "$release_directory"
  lkit-fixture-release \
    --version "$version" \
    --scenario "$scenario" \
    --ready-delay-ms "$ready_delay" \
    --native-architecture "$native_architecture" \
    --native-binary /usr/local/bin/landscape-webserver \
    --stamp-version \
    --output "$release_directory"
  AWS_ACCESS_KEY_ID="$access_key" \
  AWS_SECRET_ACCESS_KEY="$secret_key" \
  AWS_REGION="$region" \
  lkit-publish \
    --version "$version" \
    --directory "$release_directory" \
    --endpoint "$endpoint" \
    --bucket "$bucket" \
    --public-base-url "$public_base"
  local stable
  stable=$(public_get "$public_base/channels/stable.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])')
  [[ $stable == "$version" ]] || fail "stable expected $version, got $stable"
}

# 执行 switch 并把退出码放入全局 switch_status;stdout/stderr 直接进入场景日志。
run_switch() {
  local version=$1
  shift
  set +e
  lkit switch --version "$version" "$@"
  switch_status=$?
  set -e
}

# 执行 install 并把退出码放入全局 install_status。
run_install() {
  local root=$1
  shift
  set +e
  lkit install --install-dir "$root" "$@"
  install_status=$?
  set -e
}

# 把本次安装使用的仓库来源写入 config.toml(自 0.1.4 起 install 不再持久化来源,
# repair/switch/update 等需要仓库的命令从该用户可编辑文件解析来源)。
write_repository_config() {
  local root=$1
  cat >"$root/config.toml" <<EOF
schema_version = 1

[repository]
kind = "http"
location = "$public_base"
EOF
  chmod 0600 "$root/config.toml"
}

case $(uname -m) in
  x86_64)
    native_architecture=x86_64
    state_architecture=x86_64
    ;;
  aarch64|arm64)
    native_architecture=aarch64
    state_architecture=aarch64
    ;;
  *) fail "unsupported runner architecture $(uname -m)" ;;
esac

mkdir -p "$work_directory"
mkdir -p "$host_root/units" "$host_root/run/systemd/system" "$host_root/systemd-state"
printf 'nameserver 192.0.2.53\noptions timeout:1\n' >"$fixture_resolv_conf"
chmod 0644 "$fixture_resolv_conf"
python3 - "$systemctl_config" "$runtime_config" "$host_root" "$fixture_resolv_conf" <<'PY'
import json
import os
import sys

systemctl_config, runtime_config, host_root, resolv_conf = sys.argv[1:]
with open(systemctl_config, "w", encoding="utf-8") as stream:
    json.dump({
        "schema_version": 1,
        "unit_dir": os.path.join(host_root, "units"),
        "state_dir": os.path.join(host_root, "systemd-state"),
        "landscape_config": None,
        "log_path": os.path.join(host_root, "landscape.log"),
        "call_log": os.path.join(host_root, "systemctl-calls.jsonl"),
        "systemd_version": "257.fixture",
    }, stream, indent=2)
    stream.write("\n")
with open(runtime_config, "w", encoding="utf-8") as stream:
    json.dump({
        "schema_version": 1,
        "allow_non_root": True,
        "preflight": "skip",
        "execution": "inline",
        "managed_uid": os.geteuid(),
        "os_release_path": "/etc/os-release",
        "systemd": {
            "systemctl": "/usr/local/bin/lkit-test-systemctl",
            "system_unit_dir": os.path.join(host_root, "units"),
            "run_systemd_dir": os.path.join(host_root, "run/systemd/system"),
            "pid1_is_systemd": True,
            "resolv_conf": resolv_conf,
        },
        "health": {
            "base_url": "https://127.0.0.1:6443",
            "dns_tcp_port": 53,
            "dns_udp_port": 53,
            "http_port": 6300,
            "https_port": 6443,
            "startup_timeout_ms": 4000,
            "stable_duration_ms": 1200,
        },
        "export_base_url": "https://127.0.0.1:6443",
    }, stream, indent=2)
    stream.write("\n")
PY
rustfs_ip=$(getent ahostsv4 rustfs | awk 'NR == 1 {print $1}')
[[ -n "$rustfs_ip" ]] || fail "unable to resolve RustFS container"
endpoint=${endpoint/rustfs/$rustfs_ip}
# lkit 只接受 loopback 地址上的 HTTP 仓库;通过容器内 127.0.0.1 代理访问 RustFS。
# 代理必须脱离场景脚本的 stdout 管道,否则 tee 永远不会看到 EOF。
proxy_port=9000
python3 /usr/local/lib/lkit-e2e/rustfs-proxy.py \
  127.0.0.1 "$proxy_port" "$rustfs_ip" "$proxy_port" >/dev/null 2>&1 &
proxy_pid=$!
public_base="http://127.0.0.1:$proxy_port/$bucket"
resolv_sha_before=$(sha256sum "$fixture_resolv_conf" | awk '{print $1}')
resolv_mode_before=$(stat -c '%a' "$fixture_resolv_conf")
printf 'Secret123\n' >"$password_file"
chmod 0600 "$password_file"

rustfs_wait_create_bucket \
  "$endpoint" "$bucket" "$access_key" "$secret_key" "$region" 90 \
  || fail "RustFS did not become ready"
policy_file=$work_directory/policy.json
rustfs_write_public_read_policy "$policy_file" "$bucket"
signed_curl --request PUT --header 'Content-Type: application/json' \
  --data-binary @"$policy_file" --output /dev/null "$endpoint/$bucket?policy="

# ---------------------------------------------------------------- 基础生命周期
publish_release 1.0.0 healthy
lkit install \
  --version 1.0.0 \
  --repository "$public_base" \
  --install-dir "$install_root" \
  --admin-user admin \
  --password-file "$password_file"
assert_state_version 1.0.0
[[ $(json_value "$territory_root/state/install-state.json" assets.webserver.architecture) == "$state_architecture" ]] \
  || fail "state architecture mismatch"
assert_service_identity
write_repository_config "$territory_root"
[[ $(stat -c '%a' "$install_root/data/landscape_api_token") == 400 ]] \
  || fail "fixture API token mode is not 0400"

# ---------------------------------------------------------------- S1 repair 全流程
state="$territory_root/state/install-state.json"
binary="$install_root/releases/1.0.0/landscape-webserver"
corrupted_binary="$work_directory/corrupted-landscape-webserver"
cp --preserve=mode "$binary" "$corrupted_binary"
printf 'corrupted' >>"$corrupted_binary"
mv "$corrupted_binary" "$binary"
set +e
lkit repair binary
repair_status=$?
set -e
[[ $repair_status -eq 0 ]] || fail "repair binary returned $repair_status"
expected_sha=$(json_value "$state" assets.webserver.sha256)
actual_sha=$(sha256sum "$install_root/releases/1.0.0/landscape-webserver" | awk '{print $1}')
[[ $actual_sha == "$expected_sha" ]] || fail "repaired binary sha mismatch"
assert_service_identity
repair_backup=$(latest_backup)
assert_backup_metadata "$repair_backup" 1.0.0 "$state_architecture"
static_repair_pid_before=$(systemctl show --property=MainPID --value landscape-router.service)
static_repair_lkb_before=$(lkb_count)
rm -f "$install_root/current/static/index.html" "$install_root/current/static/lkit-fixture.json"
set +e
lkit repair static
repair_status=$?
set -e
[[ $repair_status -eq 0 ]] || fail "repair static returned $repair_status"
[[ -f "$install_root/current/static/index.html" ]] \
  || fail "static index.html was not restored"
[[ -f "$install_root/current/static/lkit-fixture.json" ]] \
  || fail "static lkit-fixture.json was not restored"
systemctl is-active --quiet landscape-router.service \
  || fail "service is not running after static repair"
static_repair_pid_after=$(systemctl show --property=MainPID --value landscape-router.service)
[[ $static_repair_pid_after == "$static_repair_pid_before" ]] \
  || fail "static repair restarted the service: MainPID changed from $static_repair_pid_before to $static_repair_pid_after"
[[ $(lkb_count) -eq "$static_repair_lkb_before" ]] \
  || fail "static repair must not create a .lkb backup"
assert_latest_phase committed

# ---------------------------------------------------------------- 2.0.0 成功切换
printf '\nfixture_marker = "before-2.0.0"\n' >>"$install_root/data/landscape.toml"
curl --fail --silent --show-error --insecure \
  --header 'Authorization: Bearer lkit-fixture-api-token' \
  https://127.0.0.1:6443/api/v1/system/config/export \
  | python3 -c 'import json,sys; assert "before-2.0.0" in json.load(sys.stdin)["data"]["content"]'

publish_release 2.0.0 healthy
run_switch 2.0.0
[[ $switch_status -eq 0 ]] || fail "switch to 2.0.0 returned $switch_status"
assert_state_version 2.0.0
assert_service_identity
grep -q 'before-2.0.0' "$install_root/data/landscape.toml" \
  || fail "user config marker was lost during successful switch"
assert_latest_phase committed
backup=$(latest_backup)
assert_backup_metadata "$backup" 1.0.0 "$state_architecture"
sha_v1=$(sha256sum "$install_root/releases/1.0.0/landscape-webserver" | awk '{print $1}')
sha_v2=$(sha256sum "$install_root/releases/2.0.0/landscape-webserver" | awk '{print $1}')
[[ $sha_v1 != "$sha_v2" ]] || fail "1.0.0 and 2.0.0 fixture binaries have the same sha"

# ---------------------------------------------------------------- 3.0.0 健康失败回滚
printf '\nfixture_marker_rollback = "before-3.0.0"\n' >>"$install_root/data/landscape.toml"
publish_release 3.0.0 health-error
run_switch 3.0.0
[[ $switch_status -eq 5 ]] || fail "failed switch expected exit 5, got $switch_status"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase rolled_back
backup=$(latest_backup)
assert_backup_metadata "$backup" 2.0.0 "$state_architecture"
grep -q 'before-3.0.0' "$install_root/data/landscape_init.toml" \
  || fail "restored init config is missing rollback marker"
grep -q 'before-3.0.0' "$install_root/data/landscape.toml" \
  || fail "runtime config was not initialized from restored init config"
[[ $(sha256sum "$fixture_resolv_conf" | awk '{print $1}') == "$resolv_sha_before" ]] \
  || fail "fixture resolv.conf content was not restored"
[[ $(stat -c '%a' "$fixture_resolv_conf") == "$resolv_mode_before" ]] \
  || fail "fixture resolv.conf mode was not restored"
systemctl restart landscape-router.service
assert_service_identity

# ---------------------------------------------------------------- S3 失败启动矩阵
# S2 稍后需要先运行 export_error 的 4.0.0；在更高版本前发布以保持
# stable 通道单调递增。
publish_release 4.0.0 export-error
# S3a: start_exit —— systemd start 后进程立即退出
publish_release 4.1.0 start-exit
run_switch 4.1.0
[[ $switch_status -eq 5 ]] || fail "start_exit switch expected exit 5, got $switch_status"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase rolled_back
backup=$(latest_backup)
assert_backup_metadata "$backup" 2.0.0 "$state_architecture"
[[ $(sha256sum "$fixture_resolv_conf" | awk '{print $1}') == "$resolv_sha_before" ]] \
  || fail "fixture resolv.conf content was not restored after start_exit rollback"

# S3b: exit_during_stability —— 就绪后稳定观察期退出
publish_release 4.2.0 exit-during-stability
run_switch 4.2.0
[[ $switch_status -eq 5 ]] || fail "exit_during_stability switch expected exit 5, got $switch_status"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase rolled_back
backup=$(latest_backup)
assert_backup_metadata "$backup" 2.0.0 "$state_architecture"
[[ $(sha256sum "$fixture_resolv_conf" | awk '{print $1}') == "$resolv_sha_before" ]] \
  || fail "fixture resolv.conf content was not restored after stability rollback"

# S3c: delayed_ready —— ready_delay_ms 超过测试运行时的 4 秒启动超时
publish_release 4.3.0 delayed-ready 10000
run_switch 4.3.0
[[ $switch_status -eq 5 ]] || fail "delayed_ready switch expected exit 5, got $switch_status"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase rolled_back
backup=$(latest_backup)
assert_backup_metadata "$backup" 2.0.0 "$state_architecture"
[[ $(sha256sum "$fixture_resolv_conf" | awk '{print $1}') == "$resolv_sha_before" ]] \
  || fail "fixture resolv.conf content was not restored after delayed_ready rollback"

# ---------------------------------------------------------------- S10 手工备份与恢复
# 运行中的 systemd 实例创建手工 minimal 备份(auto: false + remark),
# 列出/查看/校验,再同版本 restore(保护备份 + restore 事务提交)。
manual_remark="manual e2e backup"
set +e
lkit backup create --remark "$manual_remark"
backup_status=$?
set -e
[[ $backup_status -eq 0 ]] || fail "backup create returned $backup_status"
manual_backup=$(latest_backup)
manual_id=$(basename "$manual_backup" .lkb)
assert_manual_backup_metadata "$manual_backup" 2.0.0 "$state_architecture" "$manual_remark"
lkit backup list | grep -q "$manual_id" \
  || fail "backup list does not contain $manual_id"
lkit backup show --backup "$manual_id" \
  | grep -q "backup_id: $manual_id" \
  || fail "backup show does not print the backup id"
lkit backup verify --backup "$manual_id" \
  || fail "backup verify failed for $manual_id"
restore_lkb_before=$(lkb_count)
state_before="$territory_root/state/install-state.json"
config_before="$territory_root/config.toml"
if [[ -f "$config_before" ]]; then
  config_bytes_before=$(sha256sum "$config_before" | awk '{print $1}')
else
  config_bytes_before=
fi
lkit restore --backup "$manual_id" --non-interactive --yes \
  || fail "same-version restore returned nonzero"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase committed
[[ $(lkb_count) -eq $((restore_lkb_before + 1)) ]] \
  || fail "restore must create a protection backup"
if [[ -n "$config_bytes_before" ]]; then
  [[ $(sha256sum "$config_before" | awk '{print $1}') == "$config_bytes_before" ]] \
    || fail "restore must not modify config.toml"
else
  [[ ! -e "$config_before" ]] || fail "restore must not create config.toml"
fi
transaction=$(latest_transaction)
python3 - "$transaction" "$manual_id" <<'PY' || fail "restore transaction shape is wrong"
import json
import sys

path, backup_id = sys.argv[1:]
with open(path, encoding="utf-8") as stream:
    tx = json.load(stream)
assert tx["operation"] == "restore"
assert tx["phase"] == "committed"
assert tx["restore_backup"]["backup_id"] == backup_id
assert tx["backup"] is not None, "restore must record the protection backup"
assert tx["static_backup"] is None
PY
python3 - "$manual_backup" "$state_before" <<'PY' || fail "restored state assets do not match the backup"
import gzip
import hashlib
import io
import json
import sys
import tarfile

backup, state_path = sys.argv[1:]
with open(backup, "rb") as stream:
    content = stream.read()
metadata_length = int.from_bytes(content[6:10], "little")
archive = content[1024 * 1024:]
with tarfile.open(fileobj=io.BytesIO(gzip.decompress(archive))) as tar:
    member = tar.extractfile("static.zip")
    static_sha = hashlib.sha256(member.read()).hexdigest()
with open(state_path, encoding="utf-8") as stream:
    state = json.load(stream)
assert state["assets"]["static_archive"]["sha256"] == static_sha
PY
systemctl restart landscape-router.service
assert_service_identity

# ---------------------------------------------------------------- S4 停止服务后切换
publish_release 5.0.0 healthy
systemctl stop landscape-router.service
if systemctl is-active --quiet landscape-router.service; then
  fail "service must be inactive after systemctl stop"
fi
# 默认拒绝:要求用户先启动服务
run_switch 5.0.0
[[ $switch_status -ne 0 ]] || fail "switch to 5.0.0 must refuse a stopped service"
assert_state_version 2.0.0
assert_no_unfinished "$territory_root"
if systemctl is-active --quiet landscape-router.service; then
  fail "refused switch must not start the service"
fi
# --allow-no-backup:明确警告无备份后继续切换
lkb_before=$(lkb_count)
run_switch 5.0.0 --allow-no-backup
[[ $switch_status -eq 0 ]] || fail "no-backup switch to 5.0.0 returned $switch_status"
assert_state_version 5.0.0
assert_service_identity
pid=$(systemctl show --property=MainPID --value landscape-router.service)
[[ $(readlink -f "/proc/$pid/exe") == "$install_root/releases/5.0.0/landscape-webserver" ]] \
  || fail "MainPID does not belong to releases/5.0.0"
assert_latest_phase committed
[[ $(lkb_count) -eq "$lkb_before" ]] \
  || fail "no .lkb backup must be created by --allow-no-backup"
transaction=$(latest_transaction)
python3 - "$transaction" <<'PY' || fail "no-backup transaction must not record a backup"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    tx = json.load(stream)
assert tx["no_backup"] is True
assert tx["backup"] is None
PY

# ---------------------------------------------------------------- S9 reconcile
# a) complete 后外部修改初始化文件 → reconcile 忽略内容变化并保持文件原样
printf '\nreconcile_marker = "external"\n' >>"$install_root/data/landscape_init.toml"
init_sha_before=$(sha256sum "$install_root/data/landscape_init.toml" | awk '{print $1}')
set +e
lkit reconcile
reconcile_status=$?
set -e
[[ $reconcile_status -eq 0 ]] || fail "reconcile after init file change returned $reconcile_status"
[[ $(sha256sum "$install_root/data/landscape_init.toml" | awk '{print $1}') == "$init_sha_before" ]] \
  || fail "reconcile modified the retained initialization file"
python3 - "$state" <<'PY' || fail "install state must not record the initialization file checksum"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    state = json.load(stream)
assert "config_sha256" not in state["initialization"]
PY
# b) 再次执行:无变化,无需确认直接通过
set +e
lkit reconcile
reconcile_status=$?
set -e
[[ $reconcile_status -eq 0 ]] || fail "second reconcile returned $reconcile_status"
# c) 删除状态文件 → reconcile 拒绝重建而不是猜测
cp "$state" "$work_directory/install-state.backup"
rm "$state"
set +e
lkit reconcile
reconcile_status=$?
set -e
[[ $reconcile_status -ne 0 ]] || fail "reconcile must refuse a missing install state"
mv "$work_directory/install-state.backup" "$state"
# d) 破坏 current 链接 → reconcile 检测激活漂移并拒绝
rm "$install_root/current"
ln -s "releases/2.0.0" "$install_root/current"
set +e
lkit reconcile
reconcile_status=$?
set -e
[[ $reconcile_status -ne 0 ]] || fail "reconcile must reject an activation drift"
rm "$install_root/current"
ln -s "releases/5.0.0" "$install_root/current"
set +e
lkit reconcile
reconcile_status=$?
set -e
[[ $reconcile_status -eq 0 ]] || fail "reconcile after restoring current returned $reconcile_status"
assert_service_identity
# ---------------------------------------------------------------- S11 restore 激活失败自动回滚
# RST-03:发布 delayed-ready 版本(启动延迟 2500ms),用默认 4 秒启动超时正常切换并
# 创建其手工备份;restore 时改用 2000ms 启动超时的运行时,激活必然超时失败,
# systemd 模式内联自动回滚并返回退出码 5。
# 主安装根自基础安装起一直持有 systemd 托管,无需释放或重新注册。
assert_service_identity
publish_release 8.0.0 delayed-ready 2500
run_switch 8.0.0
[[ $switch_status -eq 0 ]] || fail "delayed_ready switch to 8.0.0 returned $switch_status"
assert_state_version 8.0.0
assert_service_identity
assert_latest_phase committed
rst03_switch_backup=$(latest_backup)
assert_backup_metadata "$rst03_switch_backup" 5.0.0 "$state_architecture"
lkit backup create --remark "rst03 target backup"
rst03_backup=$(latest_backup)
rst03_id=$(basename "$rst03_backup" .lkb)
assert_manual_backup_metadata "$rst03_backup" 8.0.0 "$state_architecture" "rst03 target backup"
# 回滚恢复的是"切换前版本"即 8.0.0 自身(delayed-ready 无法通过 2 秒启动检查);
# 先用 5.0.0 自动备份降级回健康版本(switch 拒绝降级,restore 允许),
# 让目标 restore 激活失败后回滚并重启健康旧版本。
lkit restore --backup "$(basename "$rst03_switch_backup" .lkb)" \
 --non-interactive --yes \
  || fail "setup restore back to 5.0.0 returned nonzero"
assert_state_version 5.0.0
assert_service_identity
assert_latest_phase committed
python3 - "$runtime_config" "$work_directory/restore-short-startup.json" <<'PY' || fail "failed to derive restore runtime"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    runtime = json.load(stream)
runtime["health"]["startup_timeout_ms"] = 2000
with open(sys.argv[2], "w", encoding="utf-8") as stream:
    json.dump(runtime, stream, indent=2)
    stream.write("\n")
PY
rst03_lkb_before=$(lkb_count)
set +e
command /usr/local/bin/lkit restore --backup "$rst03_id" \
  --non-interactive --yes --test-runtime "$work_directory/restore-short-startup.json"
restore_status=$?
set -e
[[ $restore_status -eq 5 ]] \
  || fail "restore activation failure expected exit 5, got $restore_status"
assert_state_version 5.0.0
assert_service_identity
assert_latest_phase rolled_back
[[ $(lkb_count) -eq $((rst03_lkb_before + 1)) ]] \
  || fail "failed restore must still create a protection backup"
[[ $(sha256sum "$fixture_resolv_conf" | awk '{print $1}') == "$resolv_sha_before" ]] \
  || fail "fixture resolv.conf content was not restored after restore rollback"
transaction=$(latest_transaction)
python3 - "$transaction" "$rst03_id" <<'PY' || fail "rolled-back restore transaction shape is wrong"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    tx = json.load(stream)
assert tx["operation"] == "restore"
assert tx["phase"] == "rolled_back"
assert tx["restore_backup"]["backup_id"] == sys.argv[2]
assert tx["backup"] is not None, "failed restore must record the protection backup"
assert tx["static_backup"] is None
PY

# ---------------------------------------------------------------- S12 restore 中断后 phase 恢复
# RST-05:恢复目标激活期间 kill 掉 lkit,事务停在 verifying,data 已移入 previous-data;
# 下次命令经 phase 恢复入口完成回滚并恢复原 data。
printf 'rst05-marker\n' >"$install_root/data/rst05-marker"
set +e
command /usr/local/bin/lkit restore --backup "$rst03_id" \
  --non-interactive --yes --test-runtime "$work_directory/restore-short-startup.json" &
restore_pid=$!
set -e
python3 - "$territory_root" "$restore_pid" <<'PY' || fail "restore did not reach the verifying phase"
import json
import os
import subprocess
import sys
import time

root, pid = sys.argv[1], int(sys.argv[2])
transactions = os.path.join(root, "transactions")
deadline = time.time() + 30
while time.time() < deadline:
    files = subprocess.run(
        ["find", transactions, "-maxdepth", "1", "-type", "f", "-name", "*.json"],
        capture_output=True,
        text=True,
    ).stdout.split()
    active = None
    for path in files:
        with open(path, encoding="utf-8") as stream:
            tx = json.load(stream)
        if tx["phase"] not in ("committed", "rolled_back", "failed"):
            if active is None or os.path.getmtime(path) > os.path.getmtime(active):
                active = path
    if active is None:
        time.sleep(0.05)
        continue
    with open(active, encoding="utf-8") as stream:
        tx = json.load(stream)
    if tx["phase"] == "verifying":
        # verifying 必然发生在 data 移入 previous-data 之后;确认落盘后杀死 lkit。
        time.sleep(0.2)
        os.kill(pid, 9)
        sys.exit(0)
    time.sleep(0.05)
sys.exit(1)
PY
wait "$restore_pid" 2>/dev/null || true
transaction=$(latest_transaction)
python3 - "$transaction" <<'PY' || fail "killed restore must leave a non-terminal transaction"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    tx = json.load(stream)
assert tx["operation"] == "restore"
assert tx["phase"] in ("activating", "verifying", "rolling_back")
PY
lkit reconcile
[[ $? -eq 0 ]] || fail "reconcile recovery after interrupted restore returned nonzero"
assert_state_version 5.0.0
assert_service_identity
assert_latest_phase rolled_back
[[ -f "$install_root/data/rst05-marker" ]] \
  || fail "previous data was not restored after interrupted restore recovery"
assert_no_unfinished "$territory_root"

# ---------------------------------------------------------------- S13 systemd 跨版本 restore
# RST-02:当前 5.0.0,用 S10 创建的 2.0.0 手工备份降级 restore,
# 不经过仓库下载;保护备份、事务提交、config.toml 来源记录保持不变。
restore_lkb_before=$(lkb_count)
config_before="$territory_root/config.toml"
if [[ -f "$config_before" ]]; then
  config_bytes_before=$(sha256sum "$config_before" | awk '{print $1}')
else
  config_bytes_before=
fi
lkit restore --backup "$manual_id" --non-interactive --yes \
  || fail "cross-version restore to 2.0.0 returned nonzero"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase committed
[[ $(lkb_count) -eq $((restore_lkb_before + 1)) ]] \
  || fail "cross-version restore must create a protection backup"
if [[ -n "$config_bytes_before" ]]; then
  [[ $(sha256sum "$config_before" | awk '{print $1}') == "$config_bytes_before" ]] \
    || fail "cross-version restore must not modify config.toml"
else
  [[ ! -e "$config_before" ]] || fail "cross-version restore must not create config.toml"
fi
transaction=$(latest_transaction)
python3 - "$transaction" "$manual_id" <<'PY' || fail "cross-version restore transaction shape is wrong"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    tx = json.load(stream)
assert tx["operation"] == "restore"
assert tx["phase"] == "committed"
assert tx["from_version"] == "5.0.0"
assert tx["target_version"] == "2.0.0"
assert tx["restore_backup"]["backup_id"] == sys.argv[2]
assert tx["backup"] is not None, "cross-version restore must record the protection backup"
assert tx["no_backup"] is False
PY
python3 - "$manual_backup" "$territory_root/state/install-state.json" <<'PY' || fail "cross-version state assets do not match the backup"
import gzip
import hashlib
import io
import json
import sys
import tarfile

backup, state_path = sys.argv[1:]
with open(backup, "rb") as stream:
    content = stream.read()
metadata_length = int.from_bytes(content[6:10], "little")
metadata = json.loads(content[32:32 + metadata_length])
archive = content[1024 * 1024:]
with tarfile.open(fileobj=io.BytesIO(gzip.decompress(archive))) as tar:
    member = tar.extractfile("landscape-webserver")
    binary_sha = hashlib.sha256(member.read()).hexdigest()
    member = tar.extractfile("static.zip")
    static_sha = hashlib.sha256(member.read()).hexdigest()
with open(state_path, encoding="utf-8") as stream:
    state = json.load(stream)
assert state["active_version"] == metadata["landscape_version"]
assert state["assets"]["webserver"]["sha256"] == binary_sha
assert state["assets"]["static_archive"]["sha256"] == static_sha
PY

# ---------------------------------------------------------------- S14 可信残留 release 复用
# INS-11/SW-11/UP-09:失败切换回滚后残留的 releases/<target> 目录在再次切换时被
# 直接复用(可信校验通过),不重复下载、不覆盖。delayed-ready 2500 在默认 4 秒启动
# 超时下成功、在 2 秒超时下失败回滚,恰好制造"下载完成但激活失败"的残留目录。
publish_release 9.0.0 delayed-ready 2500
python3 - "$runtime_config" "$work_directory/switch-short-startup.json" <<'PY' || fail "failed to derive switch runtime"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    runtime = json.load(stream)
runtime["health"]["startup_timeout_ms"] = 2000
with open(sys.argv[2], "w", encoding="utf-8") as stream:
    json.dump(runtime, stream, indent=2)
    stream.write("\n")
PY
set +e
command /usr/local/bin/lkit switch --version 9.0.0 \
  --test-runtime "$work_directory/switch-short-startup.json"
reuse_first_status=$?
set -e
[[ $reuse_first_status -eq 5 ]] \
  || fail "short-timeout switch to 9.0.0 expected exit 5, got $reuse_first_status"
assert_state_version 2.0.0
assert_service_identity
assert_latest_phase rolled_back
[[ -d "$install_root/releases/9.0.0" ]] \
  || fail "9.0.0 release directory is missing after rollback"
reuse_sha_before=$(sha256sum "$install_root/releases/9.0.0/landscape-webserver" | awk '{print $1}')
reuse_files_before=$(find "$install_root/releases/9.0.0" -type f | wc -l)

run_switch 9.0.0
[[ $switch_status -eq 0 ]] || fail "reused switch to 9.0.0 returned $switch_status"
assert_state_version 9.0.0
assert_service_identity
assert_latest_phase committed
[[ $(sha256sum "$install_root/releases/9.0.0/landscape-webserver" | awk '{print $1}') == "$reuse_sha_before" ]] \
  || fail "reused release directory was rewritten"
[[ $(find "$install_root/releases/9.0.0" -type f | wc -l) == "$reuse_files_before" ]] \
  || fail "reused release directory file set changed"

# ---------------------------------------------------------------- S7 latest 通道安装
# 全局 unit 只能属于一个安装根;先 uninstall 主安装根释放注册链接与端口
# (保留 data、config.toml 与 backups,后续场景不再使用该根)。
lkit uninstall --yes
run_install "$install_root_latest" \
  --repository "$public_base" \
  --admin-user admin \
  --password-file "$password_file"
[[ $install_status -eq 0 ]] || fail "latest-channel install returned $install_status"
assert_state_version 9.0.0 "$install_root_latest"
python3 - "$territory_root/state/install-state.json" <<'PY' \
  || fail "install-state.json must not record the repository source"
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    state = json.load(stream)
assert "repository" not in state, state.get("repository")
PY
[[ ! -e "$territory_root/config.toml" ]] \
  || fail "install must not create config.toml"
write_repository_config "$territory_root"
assert_service_identity "$install_root_latest"
# ---------------------------------------------------------------- S8 中断事务恢复
# 手工制造 preparing 现场:switch 事务文件 + 半成品目标 release 目录 + current 未动
tx_id=$(python3 -c 'import uuid; print(uuid.uuid4())')
fabricated="$territory_root/transactions/$tx_id.json"
mkdir -p "$territory_root/transactions" "$territory_root/logs"
python3 - "$fabricated" "$tx_id" "$install_root_latest" <<'PY'
import json
import sys
from datetime import datetime, timezone

path, tx_id, root = sys.argv[1:]
now = datetime.now(timezone.utc).isoformat()
transaction = {
    "schema_version": 1,
    "transaction_id": tx_id,
    "operation": "switch",
    "phase": "preparing",
    "install_root": root,
    "canonical_install_root": root,
    "from_version": "9.0.0",
    "target_version": "10.0.0",
    "from_service_manager": None,
    "target_service_manager": None,
    "previous_current": "releases/9.0.0",
    "target_release": "releases/10.0.0",
    "backup": None,
    "no_backup": False,
    "static_backup": None,
    "systemd_before": None,
    "resolv_conf_backup": None,
    "log_path": f"logs/{tx_id}.log",
    "started_at": now,
    "updated_at": now,
}
with open(path, "w", encoding="utf-8") as stream:
    json.dump(transaction, stream, indent=2)
PY
printf 'phase: preparing\n' >"$territory_root/logs/$tx_id.log"
chmod 0600 "$territory_root/logs/$tx_id.log"
mkdir -p "$install_root_latest/releases/10.0.0"
printf 'partial release' >"$install_root_latest/releases/10.0.0/landscape-webserver"

publish_release 10.0.0 healthy
run_switch 10.0.0
[[ $switch_status -eq 0 ]] || fail "recovery plus switch to 10.0.0 returned $switch_status"
[[ $(json_value "$fabricated" phase) == failed ]] \
  || fail "fabricated preparing transaction was not marked failed"
assert_no_unfinished "$territory_root"
assert_state_version 10.0.0 "$install_root_latest"
assert_latest_phase committed
backup=$(latest_backup)
assert_backup_metadata "$backup" 9.0.0 "$state_architecture"
assert_service_identity "$install_root_latest"
# ---------------------------------------------------------------- S2 导出失败回滚路径
# export 发生在备份创建之前,且总是查询运行中的服务;因此先成功安装并运行
# export_error 的 4.0.0,再尝试切换到尚未安装的 4.1.0,触发导出 500 失败。
# latest 根已持有全局 unit,先 uninstall 释放注册链接与端口。
lkit uninstall --yes
run_install "$install_root_export" \
  --version 4.0.0 \
  --repository "$public_base" \
  --admin-user admin \
  --password-file "$password_file"
[[ $install_status -eq 0 ]] || fail "export_error install returned $install_status"
assert_state_version 4.0.0 "$install_root_export"
write_repository_config "$territory_root"
assert_service_identity "$install_root_export"
lkb_before=$(lkb_count "$install_root_export")
run_switch 4.1.0
[[ $switch_status -ne 0 ]] || fail "switch from export_error must fail at export"
assert_state_version 4.0.0 "$install_root_export"
assert_latest_phase failed
[[ $(lkb_count "$install_root_export") -eq "$lkb_before" ]] \
  || fail "failed export must not create a new .lkb"
assert_service_identity "$install_root_export"

echo "PASS: Docker functional E2E completed for $native_architecture"
