#!/usr/bin/env bash
# Cross-compile the shipped Linux binaries; target set matches landscape.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

target=${1:-}
if [[ -z $target ]]; then
  case "$(uname -m)" in
    x86_64|amd64) target=x86_64-unknown-linux-musl ;;
    aarch64|arm64) target=aarch64-unknown-linux-musl ;;
    riscv64) target=riscv64gc-unknown-linux-musl ;;
    loongarch64) target=loongarch64-unknown-linux-musl ;;
    *) echo 'Unsupported host architecture; specify a supported musl target' >&2; exit 1 ;;
  esac
fi
case "$target" in
  x86_64-unknown-linux-musl) arch=x86_64; machine='Advanced Micro Devices X86-64' ;;
  aarch64-unknown-linux-musl) arch=aarch64; machine=AArch64 ;;
  riscv64gc-unknown-linux-musl) arch=riscv64; machine='RISC-V' ;;
  loongarch64-unknown-linux-musl) arch=loongarch64; machine=LoongArch ;;
  *) echo "Unsupported musl target: $target" >&2; exit 1 ;;
esac

for tool in cargo-zigbuild rustup readelf install sha256sum; do
  command -v "$tool" >/dev/null || { echo "Missing dependency: $tool" >&2; exit 1; }
done
rustup target add "$target"
# Avoid Zig's static-PIE fallback to a dynamic binary on x86_64.
flags='-C target-feature=+crt-static -C relocation-model=static'
target_env=${target^^}
target_env=${target_env//-/_}
env "CARGO_TARGET_${target_env}_RUSTFLAGS=$flags" \
  cargo zigbuild --locked --profile dist --target "$target" \
    -p lkit-cli --bin lkit -p landscape-flare --bin lflare

output=${OUTPUT_DIR:-dist}
mkdir -p "$output"
for name in lkit lflare; do
  source="${CARGO_TARGET_DIR:-target}/$target/dist/$name"
  if [[ $name == lflare ]]; then
    destination="$output/lflare-linux-$arch"
  else
    destination="$output/lkit-$arch"
  fi
  readelf -h "$source" | grep -F 'Machine:' | grep -F "$machine"
  if readelf -lW "$source" | grep -q INTERP || readelf -dW "$source" | grep -q '(NEEDED)'; then
    echo "Not fully static: $source" >&2
    exit 1
  fi
  install -m 0755 "$source" "$destination"
  echo "$destination"
done
(cd "$output" && sha256sum lkit-* lflare-linux-* >SHA256SUMS)
