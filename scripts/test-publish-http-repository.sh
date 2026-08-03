#!/usr/bin/env bash
set -euo pipefail

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "$script_directory/lib/rustfs-test.sh"

# RustFS 容器集成测试：验证 lkit-publish 的完整发布流程。
# 该测试属于集成测试，不默认混入 cargo test，由 CI 独立 job 运行。
#
# 环境变量：
#   RUSTFS_IMAGE            必填。必须固定镜像版本或 digest，禁止使用 latest。
#   RUSTFS_TEST_ACCESS_KEY  测试 Access Key，默认 lkit-test-access-key
#   RUSTFS_TEST_SECRET_KEY  测试 Secret Key，默认 lkit-test-secret-key
#   RUSTFS_TEST_BUCKET      测试 bucket，默认 lkit-test-bucket-<时间戳>
#   RUSTFS_CONTAINER_ARGS   可选容器启动参数；默认使用镜像入口和命令
#   RUSTFS_CONTAINER_PORT   容器内 S3 端口，默认 9000
#   RUSTFS_TEST_ENV         可选，空格分隔的 KEY=VALUE，追加容器环境
#   RUSTFS_TEST_REQUIRE     非空时 Docker 不可用直接失败；否则明确跳过

image=${RUSTFS_IMAGE:-}
if [[ -z "$image" ]]; then
  if [[ -n "${RUSTFS_TEST_REQUIRE:-}" ]]; then
    echo "RUSTFS_IMAGE is required" >&2
    exit 1
  fi
  echo "SKIP: RUSTFS_IMAGE is not set, cannot run RustFS integration test" >&2
  exit 0
fi

case "$image" in
  *:latest)
    echo "RUSTFS_IMAGE must pin a version or digest, not floating latest: $image" >&2
    exit 2
    ;;
esac

for command in curl python3 zstd sha256sum cmp; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 2
  fi
done

if ! docker info >/dev/null 2>&1; then
  if [[ -n "${RUSTFS_TEST_REQUIRE:-}" ]]; then
    echo "Docker is required but unavailable" >&2
    exit 1
  fi
  echo "SKIP: Docker unavailable, cannot run RustFS integration test" >&2
  exit 0
fi

if ! docker image inspect "$image" >/dev/null 2>&1; then
  echo "Pulling $image"
  docker pull "$image"
fi

access_key=${RUSTFS_TEST_ACCESS_KEY:-lkit-test-access-key}
secret_key=${RUSTFS_TEST_SECRET_KEY:-lkit-test-secret-key}
bucket=${RUSTFS_TEST_BUCKET:-lkit-test-bucket-$(date +%s%N)}
container_args=${RUSTFS_CONTAINER_ARGS:-}
container_port=${RUSTFS_CONTAINER_PORT:-9000}
region=${AWS_REGION:-us-east-1}

work_directory=$(mktemp -d)
dist_directory=$work_directory/dist
static_directory=$work_directory/static
container_name=lkit-rustfs-test-$$
container=""
data_volume=""
endpoint=""

cleanup() {
  if [[ -n "$container" ]]; then
    docker rm -f "$container" >/dev/null 2>&1 || true
  fi
  if [[ -n "$data_volume" ]]; then
    docker volume rm "$data_volume" >/dev/null 2>&1 || true
  fi
  rm -rf "$work_directory"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

mkdir -p "$dist_directory" "$static_directory/assets"

echo "== Step 1: generating small fixtures"
printf 'landscape-webserver x86_64 fixture\n' >"$dist_directory/landscape-webserver-x86_64"
printf 'landscape-webserver aarch64 fixture\n' >"$dist_directory/landscape-webserver-aarch64"
zstd -q -19 --force "$dist_directory/landscape-webserver-x86_64" -o "$dist_directory/landscape-webserver-x86_64.zst"
zstd -q -19 --force "$dist_directory/landscape-webserver-aarch64" -o "$dist_directory/landscape-webserver-aarch64.zst"
printf '<html>lkit fixture</html>\n' >"$static_directory/index.html"
printf 'console.log(1)\n' >"$static_directory/assets/app.js"
python3 - "$static_directory" "$dist_directory/static.zip" <<'PY'
import os
import sys
import zipfile

source, output = sys.argv[1:]
with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
    for root, _, names in sorted(os.walk(source)):
        for name in sorted(names):
            full = os.path.join(root, name)
            archive.write(full, os.path.join("static", os.path.relpath(full, source)))
PY

echo "== Step 2: starting RustFS container bound to loopback"
env_flags=()
if [[ -n "${RUSTFS_TEST_ENV:-}" ]]; then
  for entry in $RUSTFS_TEST_ENV; do
    env_flags+=(--env "$entry")
  done
fi
data_volume=$(docker volume create "${container_name}-data")
container_command=()
if [[ -n "$container_args" ]]; then
  read -r -a container_command <<<"$container_args"
fi
container=$(docker run --detach --rm \
  --name "$container_name" \
  --publish "127.0.0.1::$container_port" \
  --volume "$data_volume:/data" \
  --env "RUSTFS_ACCESS_KEY=$access_key" \
  --env "RUSTFS_SECRET_KEY=$secret_key" \
  "${env_flags[@]}" \
  "$image" "${container_command[@]}")

host_port=$(docker port "$container" "$container_port" | head -n1 | awk -F: '{print $NF}' | tr -d '[:space:]')
if [[ -z "$host_port" ]]; then
  fail "unable to read container port mapping"
fi
endpoint="http://127.0.0.1:$host_port"
public_base="$endpoint/$bucket"

signed_curl() {
  rustfs_signed_curl "$endpoint" "$access_key" "$secret_key" "$region" "$@"
}

echo "== Step 3: waiting for S3 API"
if ! rustfs_wait_create_bucket \
  "$endpoint" "$bucket" "$access_key" "$secret_key" "$region" 90; then
  docker logs "$container" >&2 || true
  fail "S3 API did not become ready within 90 attempts"
fi

echo "== Step 4: configuring anonymous GetObject"
policy_file=$work_directory/policy.json
rustfs_write_public_read_policy "$policy_file" "$bucket"
signed_curl --request PUT --header "Content-Type: application/json" \
  --data-binary @"$policy_file" \
  --output /dev/null \
  "$endpoint/$bucket?policy="

public_status() {
  curl --silent --show-error --head --output /dev/null --write-out '%{http_code}' \
    "$(printf '%s?lkit_test_nonce=%s' "$1" "$(date +%s%N)")" 2>/dev/null || true
}

public_get() {
  curl --fail-with-body --silent --show-error "$1"
}

verify_stable_version() {
  local expected=$1
  local actual
  actual=$(public_get "$public_base/channels/stable.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])')
  [[ $actual == "$expected" ]] || fail "stable version expected $expected, got $actual"
}

verify_manifest() {
  local version=$1
  local manifest_path=$work_directory/verify-$version.json
  local status
  status=$(public_status "$public_base/releases/$version/manifest.json")
  [[ $status == 200 ]] || fail "manifest for $version returned $status"
  public_get "$public_base/releases/$version/manifest.json" >"$manifest_path"
  python3 - "$manifest_path" "$version" "$dist_directory" <<'PY'
import hashlib
import json
import os
import sys

manifest_path, version, dist = sys.argv[1:]
with open(manifest_path, encoding="utf-8") as stream:
    manifest = json.load(stream)
assert manifest["protocol_version"] == 1, "protocol_version"
assert manifest["version"] == version, "manifest version"
expected = {
    "landscape-webserver-x86_64.zst": manifest["assets"]["webserver"]["x86_64"],
    "landscape-webserver-aarch64.zst": manifest["assets"]["webserver"]["aarch64"],
    "static.zip": manifest["assets"]["static"],
}
for name, entry in expected.items():
    with open(os.path.join(dist, name), "rb") as stream:
        digest = hashlib.sha256(stream.read()).hexdigest()
    size = os.path.getsize(os.path.join(dist, name))
    assert entry["sha256"] == digest, f"{name} sha256"
    assert entry["size"] == size, f"{name} size"
PY
}

verify_asset() {
  local version=$1
  local name=$2
  local path=$3
  local status
  status=$(public_status "$public_base/releases/$version/$name")
  [[ $status == 200 ]] || fail "asset $name for $version returned $status"
  local downloaded=$work_directory/download-$name
  public_get "$public_base/releases/$version/$name" >"$downloaded"
  cmp "$path" "$downloaded" >/dev/null || fail "asset $name for $version differs"
}

publish() {
  local version=$1
  RUSTFS_ENDPOINT="$endpoint" \
  RUSTFS_BUCKET="$bucket" \
  RUSTFS_PUBLIC_BASE_URL="$public_base" \
  AWS_ACCESS_KEY_ID="$access_key" \
  AWS_SECRET_ACCESS_KEY="$secret_key" \
  AWS_REGION="$region" \
    "$publisher" --version "$version" --directory "$dist_directory"
}

publisher=${LKIT_PUBLISH_BIN:-target/debug/lkit-publish}
if [[ ! -x "$publisher" ]]; then
  cargo build --quiet --locked -p lkit-publish
fi

echo "== Step 5: publishing 1.2.3"
publish 1.2.3

echo "== Step 6: verifying root descriptor, manifest, assets and stable pointer"
status=$(public_status "$public_base/repository.json")
[[ $status == 200 ]] || fail "repository.json returned $status"
public_get "$public_base/repository.json" | python3 -c 'import json,sys; assert json.load(sys.stdin)["protocol_version"] == 1'
verify_manifest 1.2.3
for name in landscape-webserver-x86_64.zst landscape-webserver-aarch64.zst static.zip; do
  verify_asset 1.2.3 "$name" "$dist_directory/$name"
done
verify_stable_version 1.2.3

echo "== Step 7: publishing 2.0.0 advances stable"
publish 2.0.0
verify_manifest 2.0.0
verify_stable_version 2.0.0

echo "== Step 8: publishing 1.0.0 does not downgrade stable"
publish 1.0.0
verify_manifest 1.0.0
verify_stable_version 2.0.0

echo "== Step 9: duplicate publish is rejected"
if publish 2.0.0 >/dev/null 2>&1; then
  fail "duplicate publish of 2.0.0 should have been rejected"
fi

echo "== Step 10: failed publish does not commit manifest or stable"
mv "$dist_directory/landscape-webserver-aarch64.zst" "$work_directory/hidden.zst"
if publish 3.0.0 >/dev/null 2>&1; then
  mv "$work_directory/hidden.zst" "$dist_directory/landscape-webserver-aarch64.zst"
  fail "publish with missing asset should have failed"
fi
mv "$work_directory/hidden.zst" "$dist_directory/landscape-webserver-aarch64.zst"
status=$(public_status "$public_base/releases/3.0.0/manifest.json")
[[ $status == 404 ]] || fail "failed publish left releases/3.0.0/manifest.json ($status)"
verify_stable_version 2.0.0

echo "PASS: RustFS publish integration test completed"
