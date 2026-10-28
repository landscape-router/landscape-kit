#!/usr/bin/env bash
# Run only architecture-sensitive, side-effect-free unit modules on musl.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

target=${1:-x86_64-unknown-linux-musl}
case "$target" in
  x86_64-unknown-linux-musl) runner= ;;
  aarch64-unknown-linux-musl) runner=qemu-aarch64 ;;
  riscv64gc-unknown-linux-musl) runner=qemu-riscv64 ;;
  loongarch64-unknown-linux-musl) runner=qemu-loongarch64 ;;
  *) echo "Unsupported musl test target: $target" >&2; exit 1 ;;
esac
for tool in cargo-zigbuild rustup tee grep ${runner:+"$runner"}; do
  command -v "$tool" >/dev/null || { echo "Missing dependency: $tool" >&2; exit 1; }
done
rustup target add "$target"

target_env=${target^^}
target_env=${target_env//-/_}
export "CARGO_TARGET_${target_env}_RUSTFLAGS=-C target-feature=+crt-static -C relocation-model=static"
if [[ -n $runner ]]; then
  export "CARGO_TARGET_${target_env}_RUNNER=$runner"
else
  unset "CARGO_TARGET_${target_env}_RUNNER"
fi
# Keep emulated crypto tests fast, including debug assertions in the test profile.
export CARGO_PROFILE_TEST_OPT_LEVEL=1
export CARGO_PROFILE_TEST_DEBUG=0
export LANDSCAPE_TERRAIN_SCRYPT_LOG_N=10

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
export TMPDIR="$workdir"
export LKIT_TERRITORY="$workdir/territory"
results_dir=${MUSL_TEST_RESULTS_DIR:-$workdir/results}
mkdir -p "$results_dir"

run_tests() {
  local name=$1
  shift
  cargo-zigbuild test --locked --target "$target" "$@" -- --test-threads=1 \
    | tee "$results_dir/$name.log"
  # A stale module filter must fail CI instead of silently running zero tests.
  grep -Eq '^test result: ok\. [1-9][0-9]* passed; 0 failed;' "$results_dir/$name.log"
}

run_tests terrain-protocol -p landscape-terrain-proto --lib protocol::
# Do not enable test-support: the fixture TLS server is not needed by these
# modules and would pull an unrelated crypto backend into cross-compilation.
run_tests config -p lkit-cli --bin lkit deployment::config::tests::
run_tests archive -p lkit-cli --bin lkit release::repository::archive::tests::
run_tests backup-read -p lkit-cli --bin lkit backup::lkb::read::tests::
run_tests backup-write -p lkit-cli --bin lkit backup::lkb::write::tests::

if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
  printf '### musl functional tests: %s\n\n' "$target" >>"$GITHUB_STEP_SUMMARY"
  grep '^test result:' "$results_dir/"*.log >>"$GITHUB_STEP_SUMMARY"
fi
