#!/usr/bin/env bash
# 多发行版换源 E2E：在真实发行版容器（Debian/Ubuntu/Fedora/Arch Linux）里验证
# `lkit set-mirror` 的切换、备份、恢复与"仅 CD 源"兜底。
#
# 流程：先用 rust:bookworm 构建一次 lkit 生产二进制，通过 docker 命名卷挂载进
# 各发行版容器执行 run-distro.sh。容器内以 root 运行，无需 test-support。
# 换源只改文件不联网。命名卷避免了 docker 对缺失宿主路径的自动创建（root 目录）。
set -euo pipefail

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

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
volume=lkit-mirror-bin
cleanup() {
  docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker volume rm "$volume" >/dev/null 2>&1 || true

echo "== building lkit release binary =="
docker build --file "$root/scripts/docker-mirrors/Dockerfile" --tag lkit-mirror-builder "$root"
docker run --rm --volume "$volume:/out" lkit-mirror-builder cp /output/lkit /out/lkit
docker run --rm --volume "$volume:/out" lkit-mirror-builder test -x /out/lkit

overall=0
while read -r distro image; do
  echo "== $distro ($image) =="
  if docker run --rm \
    --volume "$volume:/usr/local/bin:ro" \
    --volume "$root/scripts/docker-mirrors/run-distro.sh:/opt/run-distro.sh:ro" \
    --entrypoint bash \
    "$image" /opt/run-distro.sh "$distro" 2>&1 | sed "s/^/[$distro] /"; then
    echo "[$distro] passed"
  else
    echo "[$distro] FAILED" >&2
    overall=1
  fi
done <<'EOF'
debian debian:bookworm
ubuntu ubuntu:24.04
fedora fedora:latest
archlinux archlinux:latest
EOF

if [[ $overall -eq 0 ]]; then
  echo "all distro mirror checks passed"
else
  echo "some distro mirror checks failed" >&2
fi
exit "$overall"
