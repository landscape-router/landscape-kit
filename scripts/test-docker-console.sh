#!/usr/bin/env bash
# 在 docker 隔离环境中运行 console 的 fixture e2e 测试。
#
# 为什么必须隔离:该套件会启动真实 `lkit` 二进制(TUI 控制台),并可能触发
# daemon/网络接管相关路径。直接在宿主机运行有破坏本机网络环境的风险。
# 本脚本分两阶段:
#   1. 有网阶段——在容器内编译 e2e 测试二进制与 fixture 辅助二进制
#      (target 缓存在 named volume 中,重复运行增量编译);
#   2. 断网阶段——`--network none` 运行测试:容器只剩 loopback(fixture 的
#      本地 HTTP 服务仍可用),物理上不可能触碰宿主机或外部网络。
#
# 用法:scripts/test-docker-console.sh [filter](默认 filter 为 console,
# 匹配 console:: 与 console_screen:: 两个模块)。

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=rust:1.97.1-bookworm
TARGET_VOLUME=lkit-console-e2e-target
CARGO_VOLUME=lkit-console-e2e-cargo
FILTER=${1:-console}

docker volume create "$TARGET_VOLUME" >/dev/null
docker volume create "$CARGO_VOLUME" >/dev/null

echo "==> [1/2] 在容器内编译 fixture e2e(需要网络拉取依赖)"
docker run --rm \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -v "$CARGO_VOLUME":/usr/local/cargo/registry \
    -w /src \
    "$IMAGE" \
    cargo test --locked -p lkit-cli --features test-support \
        --test install_fixture_e2e --no-run

echo "==> [2/2] 断网运行 filter='$FILTER'(LKIT_E2E=1)"
docker run --rm \
    --network none \
    -e LKIT_E2E=1 \
    -e "FILTER=$FILTER" \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -w /src \
    "$IMAGE" \
    bash -c '
        bin=$(find target/debug/deps -maxdepth 1 -type f \
            -name "install_fixture_e2e-*" -perm -111 | sort | tail -n 1)
        test -n "$bin" || { echo "e2e test binary not found" >&2; exit 1; }
        exec "$bin" --test-threads=1 "$FILTER"
    '
