#!/usr/bin/env bash
# 多发行版常用软件（Docker）安装 E2E：在真实发行版容器（Debian/Ubuntu/Fedora/Arch
# Linux）里验证 `lkit software install docker` 的仓库配置、GPG key、真实软件包安装
# 与服务启用契约。
#
# 流程：先用 rust:bookworm 构建一次 lkit 生产二进制，通过 docker 命名卷挂载进各发行版
# 容器执行 run-distro.sh。容器内以 root 运行，无需 test-support。与换源 E2E 不同，
# 本测试需要联网下载 docker-ce 软件包与 GPG key；容器内没有 systemd 也无法运行真实
# dockerd，服务层使用记录型 shim（见 run-distro.sh）。
set -euo pipefail

case $(uname -s):$(uname -m) in
  Linux:x86_64) ;;
  Linux:aarch64|Linux:arm64)
    if [[ ${LKIT_E2E_ALLOW_ARM:-} != 1 ]]; then
      echo "Docker software E2E is supported locally only on Linux x86_64; use CI for aarch64" >&2
      exit 2
    fi
    ;;
  *)
    echo "Docker software E2E requires Linux x86_64 or the CI ARM runner" >&2
    exit 2
    ;;
esac

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for the software E2E" >&2
  exit 1
fi

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
volume=lkit-software-bin
cleanup() {
  docker volume rm "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker volume rm "$volume" >/dev/null 2>&1 || true

echo "== building lkit release binary =="
docker build --file "$root/scripts/docker/software/Dockerfile" --tag lkit-software-builder "$root"
docker run --rm --volume "$volume:/out" lkit-software-builder cp /output/lkit /out/lkit
docker run --rm --volume "$volume:/out" lkit-software-builder test -x /out/lkit

overall=0
# Arch Linux 官方镜像只有 amd64 manifest;aarch64 job 跳过 archlinux。
distro_list=$'debian debian:bookworm\nubuntu ubuntu:24.04\nfedora fedora:latest'
if [[ $(uname -m) == x86_64 ]]; then
  distro_list+=$'\narchlinux archlinux:latest'
fi
while read -r distro image; do
  [[ -n $distro ]] || continue
  echo "== $distro ($image) =="
  if docker run --rm \
    --volume "$volume:/usr/local/bin:ro" \
    --volume "$root/scripts/docker/software/run-distro.sh:/opt/run-distro.sh:ro" \
    --entrypoint bash \
    "$image" /opt/run-distro.sh "$distro" 2>&1 | sed "s/^/[$distro] /"; then
    echo "[$distro] passed"
  else
    echo "[$distro] FAILED" >&2
    overall=1
  fi
done <<< "$distro_list"

if [[ $overall -eq 0 ]]; then
  echo "all distro software checks passed"
else
  echo "some distro software checks failed" >&2
fi
exit "$overall"
