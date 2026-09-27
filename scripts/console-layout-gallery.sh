#!/usr/bin/env bash
# 在 docker 隔离环境中生成控制台 TUI 布局 gallery,供人工/agent 审阅实际布局。
#
# 产物(写入第一个参数指定的目录,默认 .console-gallery/):
#   gallery.html            18 屏确定性渲染的彩色终端模拟页(ratatui TestBackend
#                           Buffer,与 insta 快照同一组屏幕/夹具)
#   <screen>.txt            每屏的纯文本 dump
#   real-<panel>.txt        真 PTY 中运行裸 `lkit` 逐面板遍历的解码屏幕文本
#
# 与 test-docker-console.sh 相同的两阶段隔离:有网编译、断网(--network none,
# 仅 loopback)运行;target/cargo 缓存共用同一组 named volume。

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
IMAGE=rust:1.97.1-bookworm
TARGET_VOLUME=lkit-console-e2e-target
CARGO_VOLUME=lkit-console-e2e-cargo
OUT_DIR=$(cd "$(dirname "${1:-.console-gallery}")" && pwd)/$(basename "${1:-.console-gallery}")

mkdir -p "$OUT_DIR"
docker volume create "$TARGET_VOLUME" >/dev/null
docker volume create "$CARGO_VOLUME" >/dev/null

echo "==> [1/2] 在容器内编译 gallery 与 e2e(需要网络拉取依赖)"
docker run --rm \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -v "$CARGO_VOLUME":/usr/local/cargo/registry \
    -w /src \
    "$IMAGE" \
    cargo test --locked -p lkit-cli --features test-support \
        --bin lkit console::tests::gallery --no-run
docker run --rm \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -v "$CARGO_VOLUME":/usr/local/cargo/registry \
    -w /src \
    "$IMAGE" \
    cargo test --locked -p lkit-cli --features test-support \
        --test install_fixture_e2e --no-run

echo "==> [2/2] 断网生成 gallery 产物 → $OUT_DIR"
docker run --rm \
    --network none \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e LKIT_CONSOLE_GALLERY=/out \
    -v "$OUT_DIR":/out \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -v "$CARGO_VOLUME":/usr/local/cargo/registry \
    -w /src \
    "$IMAGE" \
    cargo test --offline -p lkit-cli --features test-support \
        --bin lkit console::tests::gallery

docker run --rm \
    --network none \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e LKIT_E2E=1 \
    -e LKIT_CONSOLE_GALLERY=/out \
    -v "$OUT_DIR":/out \
    -v "$REPO_ROOT":/src \
    -v "$TARGET_VOLUME":/src/target \
    -w /src \
    "$IMAGE" \
    bash -c '
        bin=$(find target/debug/deps -maxdepth 1 -type f \
            -name "install_fixture_e2e-*" -perm -111 | sort | tail -n 1)
        test -n "$bin" || { echo "e2e test binary not found" >&2; exit 1; }
        exec "$bin" --test-threads=1 console_screen::walks_all_panels
    '

echo "==> 完成:$OUT_DIR"
ls "$OUT_DIR"
