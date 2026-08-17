#!/usr/bin/env bash
# Download the .deb packages needed by the e2e test image into scripts/flare/packages/.
# Runs on the host (where apt works); the Dockerfile then installs them
# offline, so `docker build` never needs network access.
set -euo pipefail
cd "$(dirname "$0")"

# libpcap 的包名随发行版不同：bookworm 是 libpcap0.8，trixie 与
# Ubuntu 24.04 是 libpcap0.8t64。用宿主可用者即可（运行时并不需要，
# 只是与镜像构建时保持一致）。
PKGS="iproute2 netcat-openbsd python3"
if [ -n "$(apt-cache show libpcap0.8 2>/dev/null)" ]; then
  PKGS="libpcap0.8 $PKGS"
elif [ -n "$(apt-cache show libpcap0.8t64 2>/dev/null)" ]; then
  PKGS="libpcap0.8t64 $PKGS"
fi
mkdir -p packages

list=""
for p in $PKGS; do
  deps=$(apt-cache depends --recurse --no-recommends --no-suggests \
    --no-conflicts --no-breaks --no-replaces "$p" \
    | awk '/^[[:space:]]/ { print $2 }' \
    | grep -v '<' | sort -u)
  list="$list $p $deps"
done

cd packages
apt-get download $(echo "$list" | tr ' ' '\n' | sort -u | tr '\n' ' ') 2>&1 \
  | grep -E "^(Get|E:)" || true
# 宿主启用 multiarch 时,递归依赖会混入其它架构的 .deb;dpkg -i 只认
# 本架构与 all 包,其余删除。
arch=$(dpkg --print-architecture)
find . -maxdepth 1 -name "*.deb" ! -name "*_${arch}.deb" ! -name "*_all.deb" -delete
echo "downloaded $(ls *.deb 2>/dev/null | wc -l) packages into $(pwd)"
