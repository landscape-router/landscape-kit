#!/usr/bin/env bash
# Download the .deb packages needed by the flare e2e test image into
# packages/. Runs on the host (where apt works); the Dockerfile then installs
# them offline, so `docker build` never needs network access.
set -euo pipefail
cd "$(dirname "$0")"

PKGS="libpcap0.8 iproute2 netcat-openbsd python3"
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
echo "downloaded $(ls *.deb 2>/dev/null | wc -l) packages into $(pwd)"
