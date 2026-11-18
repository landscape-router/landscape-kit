#!/usr/bin/env bash
set -euo pipefail
base=https://github.com/srl-labs/containerlab/releases/download/v0.69.3
package=containerlab_0.69.3_linux_amd64.deb
curl -fsSLO "$base/$package"
curl -fsSL "$base/checksums.txt" | grep -F "$package" | sha256sum -c -
sudo apt-get update
sudo apt-get install -y "./$package" musl-tools
rustup target add x86_64-unknown-linux-musl
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="--cfg tokio_unstable -C target-feature=+crt-static -C relocation-model=static"
cargo build --locked --target x86_64-unknown-linux-musl -p lkit-cli --bin lkit -p landscape-flare --bin lflare
for app in lkit lflare; do
  file "target/x86_64-unknown-linux-musl/debug/$app"
  if readelf -lW "target/x86_64-unknown-linux-musl/debug/$app" | grep -q INTERP; then exit 1; fi
done
