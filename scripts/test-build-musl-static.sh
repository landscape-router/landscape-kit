#!/usr/bin/env bash
# Test target selection without downloading tools or compiling binaries.
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
mkdir -p "$test_root/bin"

cat >"$test_root/bin/uname" <<'SH'
#!/bin/sh
printf '%s\n' "$TEST_HOST_ARCH"
SH
cat >"$test_root/bin/rustup" <<'SH'
#!/bin/sh
exit 0
SH
cat >"$test_root/bin/cargo" <<'SH'
#!/bin/sh
printf '%s\n' "$@" >"$TEST_CARGO_ARGS"
env >"$TEST_CARGO_ENV"
# Stop before the build script tries to stage any artifacts.
exit 73
SH
chmod +x "$test_root/bin/"*
ln -s cargo "$test_root/bin/cargo-zigbuild"

fail() { echo "build-musl-static test: $*" >&2; exit 1; }

check_target() {
  local host=$1 expected=$2 status=0 target_env
  shift 2
  env PATH="$test_root/bin:$PATH" TEST_HOST_ARCH="$host" \
    TEST_CARGO_ARGS="$test_root/args" TEST_CARGO_ENV="$test_root/env" \
    bash "$repo_root/scripts/build-musl-static.sh" "$@" >"$test_root/output" 2>&1 || status=$?
  [[ $status == 73 ]] || fail "$host: unexpected status $status"
  grep -Fx -- "$expected" "$test_root/args" >/dev/null || fail "$host: wrong target"
  target_env=${expected^^}
  target_env=${target_env//-/_}
  grep -Fx "CARGO_TARGET_${target_env}_RUSTFLAGS=-C target-feature=+crt-static -C relocation-model=static" \
    "$test_root/env" >/dev/null || fail "$host: missing static flags"
}

for host in x86_64 amd64; do check_target "$host" x86_64-unknown-linux-musl; done
for host in aarch64 arm64; do check_target "$host" aarch64-unknown-linux-musl; done
check_target riscv64 riscv64gc-unknown-linux-musl
check_target loongarch64 loongarch64-unknown-linux-musl
# An explicit cross target takes precedence over the host, even an unsupported host.
check_target s390x riscv64gc-unknown-linux-musl riscv64gc-unknown-linux-musl

for target in '' x86_64-unknown-linux-gnu; do
  rm -f "$test_root/args"
  args=()
  [[ -z $target ]] || args=("$target")
  if env PATH="$test_root/bin:$PATH" TEST_HOST_ARCH=s390x \
    TEST_CARGO_ARGS="$test_root/args" TEST_CARGO_ENV="$test_root/env" \
    bash "$repo_root/scripts/build-musl-static.sh" "${args[@]}" >"$test_root/output" 2>&1; then
    fail "unsupported host/target was accepted"
  fi
  [[ ! -f $test_root/args ]] || fail "unsupported host/target reached cargo"
done

echo 'build-musl-static test: passed'
