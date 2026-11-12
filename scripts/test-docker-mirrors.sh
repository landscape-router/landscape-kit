#!/usr/bin/env bash
# 多发行版换源 E2E：在真实发行版容器（Debian/Ubuntu/Fedora/Arch Linux）里验证
# `lkit set-mirror` 的切换、备份、恢复与"仅 CD 源"兜底。
#
# 流程：先用 rust:bookworm 构建一次 lkit 生产二进制，通过 docker 命名卷挂载进
# 各发行版容器执行 run-distro.sh。容器内以 root 运行，无需 test-support。
# Probes and index refreshes contact public mirrors: ci-mirror pushes only.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/lib/mirror-network-guard.sh
source "$root/scripts/lib/mirror-network-guard.sh"
lkit_require_mirror_network || exit $?

case $(uname -s):$(uname -m) in
  Linux:x86_64) ;;
  Linux:aarch64|Linux:arm64)
    if [[ ${LKIT_E2E_ALLOW_ARM:-} != 1 ]]; then
      echo "Docker mirror E2E is supported locally only on Linux x86_64; use CI for aarch64" >&2
      exit 2
    fi
    ;;
  *)
    echo "Docker mirror E2E requires Linux x86_64 or the CI ARM runner" >&2
    exit 2
    ;;
esac

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for the mirror E2E" >&2
  exit 1
fi

volume=lkit-mirror-bin
# Invoked indirectly by the EXIT trap.
# shellcheck disable=SC2317
cleanup() {
  docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker volume rm "$volume" >/dev/null 2>&1 || true

echo "== building lkit release binary =="
docker build --file "$root/scripts/docker/mirrors/Dockerfile" --tag lkit-mirror-builder "$root"
docker run --rm --volume "$volume:/out" lkit-mirror-builder cp /output/lkit /out/lkit
docker run --rm --volume "$volume:/out" lkit-mirror-builder test -x /out/lkit

overall=0
# Arch Linux 官方镜像只有 amd64 manifest;aarch64 job 跳过 archlinux。
# Keep both legacy and DNF5 vendor-repository layouts in the real-image matrix.
distro_list=$'debian debian:bookworm\nubuntu ubuntu:24.04\nfedora fedora:44\nfedora fedora:45'
if [[ $(uname -m) == x86_64 ]]; then
  distro_list+=$'\narchlinux archlinux:latest'
fi
while read -r distro image; do
  [[ -n $distro ]] || continue
  echo "== $distro ($image) =="
  if docker run --rm \
    --env LKIT_ALLOW_MIRROR_NETWORK --env GITHUB_ACTIONS \
    --env GITHUB_EVENT_NAME --env GITHUB_REF \
    --volume "$root/scripts/lib/mirror-network-guard.sh:/opt/mirror-network-guard.sh:ro" \
    --volume "$volume:/usr/local/bin:ro" \
    --volume "$root/scripts/docker/mirrors/run-distro.sh:/opt/run-distro.sh:ro" \
    --entrypoint bash \
    "$image" /opt/run-distro.sh "$distro" 2>&1 | sed "s/^/[$distro] /"; then
    echo "[$distro] passed"
  else
    echo "[$distro] FAILED" >&2
    overall=1
  fi
done <<< "$distro_list"

if [[ $overall -eq 0 ]]; then
  echo "all distro mirror checks passed"
else
  echo "some distro mirror checks failed" >&2
fi
exit "$overall"
