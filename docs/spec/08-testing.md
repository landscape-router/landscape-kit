# 测试策略与 E2E 架构

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 测试分层

| 层级 | 位置 | 依赖 | 运行环境 | CI 命令 |
|------|------|------|----------|---------|
| 单元测试 | `#[cfg(test)] mod tests` | 无外部依赖 | 宿主机 | `cargo test --workspace` |
| 集成测试 | crate `tests/` 目录 | mock trait / 临时目录 | 宿主机 | `cargo test --workspace` |
| E2E 测试 | `tests/e2e/scenarios/` | systemd / 文件系统 / 网络 | Docker 容器 | `bash tests/e2e/run.sh` |

### 2.1 边界定义

- **单元测试**：纯逻辑验证（编解码、校验规则、路径规范化、计划生成），不涉及 I/O
- **集成测试**：验证用例层流程，通过 trait mock 替换外部依赖（API 客户端、systemd 操作），在临时目录操作文件
- **E2E 测试**：验证真实系统交互——二进制执行、systemd 服务生命周期、文件系统布局、API 连通性。不使用 mock

### 2.2 为什么 E2E 必须独立于 mock

lkit 是系统管理工具，核心价值在于与 systemd、文件系统、网络的真实交互。mock 测试无法覆盖：

- systemd unit 文件是否被正确安装和加载
- 二进制是否能被 systemd 正确启动
- 目录权限和所有权是否符合预期
- 端到端流程中各步骤的副作用是否正确累积

## 3. E2E 架构

### 3.1 设计原则

1. **容器只提供环境**：systemd + 基础工具 + mock API，不包含 lkit 本体
2. **二进制通过 volume mount 注入**：CI 和本地共享同一构建流程，容器镜像无需随代码变更重建
3. **测试脚本 COPY 进镜像**：场景和断言库与环境绑定，保证执行一致性
4. **入口统一**：本地和 CI 执行同一条命令 `bash tests/e2e/run.sh`

### 3.2 数据流

```
宿主机                            容器 (debian:bookworm + systemd)
┌─────────────────┐              ┌─────────────────────────────┐
│ cargo build     │              │ systemd (PID 1)             │
│   └→ lkit 二进制 │──mount:ro──→│ /usr/local/bin/lkit         │
│                 │              │                             │
│ run.sh          │              │ /opt/e2e/                   │
│   └→ docker exec┼─────────────→│   lib/helpers.sh            │
│                 │              │   scenarios/01-install.sh   │
│ tests/e2e/      │              │   scenarios/02-health-check.sh
│   └→ fixtures/  │──mount:ro──→│ /fixtures/                  │
└─────────────────┘              └─────────────────────────────┘
```

### 3.3 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 容器运行时 | Docker（GitHub Actions 原生支持） | 零额外安装，本地/CI 一致 |
| 二进制传递 | volume mount，不 COPY 进镜像 | 镜像稳定不随代码变更，构建一次测多次 |
| 场景脚本位置 | COPY 进镜像 | 路径统一，无宿主机/容器路径映射心智负担 |
| 测试语言 | bash | CLI 测试本质是"执行命令 → 检查状态"，shell 天然匹配 |
| 场景间隔离 | 默认共享容器，可升级为独立容器 | 初期简单，见 3.4 |

### 3.4 场景隔离策略

两种方案：

**方案 A：共享容器（简单，适合初期）**

所有场景在同一个容器内顺序执行。场景需自行清理副作用。

- 优点：启动快（一次容器启动）
- 缺点：场景间有状态泄漏风险

**方案 B：独立容器（严格，推荐）**

每个场景启动独立容器。天然隔离，无清理负担。

- 优点：完全隔离，可并行执行
- 缺点：每个场景多 2-3 秒容器启动开销

**默认采用方案 A**，场景内自行管理前置条件（`lkit install` 前检查是否已安装）。当场景间出现干扰时升级到方案 B。

## 4. 目录结构

```text
tests/e2e/
├── run.sh                     # 入口脚本（宿主机执行）
├── Dockerfile                 # 容器环境定义
├── lib/
│   └── helpers.sh             # 断言函数库
├── fixtures/                  # 测试数据
│   ├── install/
│   │   └── init.toml          # 安装场景初始化文件
│   ├── backup/                # M3 备份场景数据
│   └── upgrade/               # M3 升级场景数据
└── scenarios/                 # 场景脚本（每个文件 = 一个场景）
    ├── 01-install.sh          # M2：基础安装
    ├── 02-health-check.sh     # M2：首次启动健康检查
    ├── 03-network-config.sh   # M2：网络配置引导
    ├── 10-backup.sh           # M3：备份创建与验证
    ├── 11-restore.sh          # M3：恢复流程
    └── 12-upgrade-rollback.sh # M3：升级与回滚
```

场景编号规则：`XY-<name>.sh`，X 为里程碑号（2=M2, 3=M3），Y 为场景序号。

## 5. 核心组件

### 5.1 入口脚本（run.sh）

职责：构建 → 启动容器 → 执行场景 → 收集结果 → 清理

```bash
#!/usr/bin/env bash
set -euo pipefail

# --- 构建阶段 ---
cargo build --release
docker build -t lkit-e2e tests/e2e/

# --- 容器生命周期 ---
CID=$(docker run -d --privileged --cgroupns=host \
  -v "$(pwd)/target/release/lkit:/usr/local/bin/lkit:ro" \
  -v "$(pwd)/tests/e2e/fixtures:/fixtures:ro" \
  lkit-e2e)

trap "docker rm -f $CID" EXIT

# --- 等待 systemd 就绪 ---
docker exec "$CID" bash -c \
  'until systemctl is-system-running 2>/dev/null | grep -qE "running|degraded"; do sleep 0.5; done'

# --- 执行场景 ---
PASSED=0 FAILED=0
for s in /opt/e2e/scenarios/*.sh; do
  name=$(basename "$s" .sh)
  if docker exec "$CID" bash "$s"; then
    echo "PASS  $name" && ((PASSED++))
  else
    echo "FAIL  $name" && ((FAILED++))
  fi
done

echo "---"
echo "$PASSED passed, $FAILED failed"
[[ $FAILED -eq 0 ]]
```

支持选择性执行：

```bash
bash tests/e2e/run.sh                           # 全部场景
bash tests/e2e/run.sh /opt/e2e/scenarios/01-*.sh # 只跑 M2 场景
```

### 5.2 容器环境（Dockerfile）

```dockerfile
FROM debian:bookworm

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      systemd-sysv dbus curl jq && \
    rm -rf /var/lib/apt/lists/*

COPY lib/ /opt/e2e/lib/
COPY scenarios/ /opt/e2e/scenarios/

STOPSIGNAL SIGRTMIN+3
ENTRYPOINT ["/sbin/init"]
```

- `systemd-sysv`：提供 systemd 作为 init 系统
- `dbus`：systemd 服务管理依赖
- `curl` / `jq`：场景中 API 验证和响应解析
- `STOPSIGNAL SIGRTMIN+3`：systemd 标准停止信号

### 5.3 断言库（lib/helpers.sh）

提供可在场景脚本中复用的断言函数：

| 函数 | 用途 |
|------|------|
| `assert_exit_code <expected>` | 验证上一条命令的退出码 |
| `assert_file_exists <path>` | 验证文件存在 |
| `assert_file_contains <file> <pattern>` | 验证文件内容包含指定模式 |
| `assert_service_active <unit>` | 验证 systemd 服务处于 active 状态 |
| `assert_service_enabled <unit>` | 验证 systemd 服务已 enabled |
| `assert_api_responds <url>` | 验证 HTTP 端点可达且返回 2xx |
| `assert_api_field <url> <jq_expr> <expected>` | 验证 API JSON 响应字段值 |

断言失败时输出诊断信息（实际值、期望值、相关文件内容），便于 CI 日志排查。

### 5.4 场景脚本示例

```bash
# scenarios/01-install.sh
#!/usr/bin/env bash
set -euo pipefail
source /opt/e2e/lib/helpers.sh

# 前置条件：Landscape 未安装
[[ ! -f /etc/landscape/landscape.toml ]] || {
  echo "SKIP: already installed"; exit 0
}

# 执行安装
lkit install --non-interactive --init-file /fixtures/install/init.toml
assert_exit_code 0

# 验证文件布局
assert_file_exists /etc/landscape/landscape.toml
assert_file_exists /opt/landscape/landscape
assert_file_exists /opt/landscape/static/index.html

# 验证 systemd 服务
assert_service_active landscape.service
assert_service_enabled landscape.service

# 验证 API 健康
assert_api_responds http://127.0.0.1:8080/api/v1/health
```

## 6. CI 集成

### 6.1 GitHub Actions 工作流

```yaml
# .github/workflows/e2e.yml
name: E2E Tests

on:
  pull_request:
    paths: ['crates/**', 'tests/e2e/**']
  workflow_dispatch:

jobs:
  e2e:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Run E2E tests
        run: bash tests/e2e/run.sh

      - name: Collect container logs on failure
        if: failure()
        run: |
          mkdir -p tests/e2e/artifacts
          CID=$(docker ps -a -l -q --filter "ancestor=lkit-e2e")
          docker logs "$CID" > tests/e2e/artifacts/container.log 2>&1 || true
          docker exec "$CID" journalctl -xb > tests/e2e/artifacts/journal.log 2>&1 || true

      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: e2e-logs
          path: tests/e2e/artifacts/
```

### 6.2 CI 性能预期

| 阶段 | 首次运行 | 后续运行（有缓存） |
|------|---------|-------------------|
| `cargo build --release` | 2-3 min | 30-60 s（增量编译） |
| `docker build` | 30-60 s（apt-get） | <5 s（层缓存） |
| 容器启动 + systemd 就绪 | 3-5 s | 3-5 s |
| 单个场景 | 5-30 s | 5-30 s |
| **M2 全部场景（3 个）** | **~4 min** | **~1.5 min** |

### 6.3 与现有 CI 的关系

```yaml
# .github/workflows/ci.yml — 不变
jobs:
  fmt:     # cargo fmt --all -- --check
  clippy:  # cargo clippy --all -- --D warnings
  test:    # cargo test --workspace

# .github/workflows/e2e.yml — 新增
jobs:
  e2e:     # bash tests/e2e/run.sh
```

E2E 工作流独立于现有 CI，按路径过滤触发。代码未变更时（纯文档/配置）不触发。

## 7. 本地开发

### 7.1 运行全部场景

```bash
bash tests/e2e/run.sh
```

### 7.2 运行单个场景

```bash
cargo build --release
docker build -t lkit-e2e tests/e2e/

CID=$(docker run -d --privileged --cgroupns=host \
  -v "$(pwd)/target/release/lkit:/usr/local/bin/lkit:ro" \
  -v "$(pwd)/tests/e2e/fixtures:/fixtures:ro" \
  lkit-e2e)

docker exec "$CID" bash /opt/e2e/scenarios/01-install.sh
```

### 7.3 交互式调试

```bash
docker exec -it "$CID" bash
# 进入容器后可手动执行 lkit 命令、查看 systemd 状态、检查文件
```

## 8. 扩展路径

### 8.1 M3 场景扩展

M3（备份/恢复/升级）在现有架构上扩展：

- 新增 `fixtures/backup/`、`fixtures/upgrade/` 测试数据目录
- 新增 `scenarios/10-*.sh`、`scenarios/11-*.sh` 场景脚本
- 升级场景需要容器内预装旧版 Landscape（Dockerfile 或场景 setup 阶段处理）

### 8.2 场景隔离升级

当场景间出现状态干扰时，将 `run.sh` 改为每个场景启动独立容器：

```bash
for s in /opt/e2e/scenarios/*.sh; do
  name=$(basename "$s" .sh)
  cid=$(docker run -d ...)       # 每个场景新容器
  docker exec "$cid" bash "$s"
  docker rm -f "$cid"
done
```

### 8.3 并行执行

方案 B（独立容器）下，场景可并行执行：

```bash
for s in /opt/e2e/scenarios/*.sh; do
  (run_scenario "$s") &          # 后台并行
done
wait
```

并行时需确保场景间无共享资源依赖（端口、文件路径）。

## 9. 里程碑覆盖

| 里程碑 | E2E 场景 | 关键断言 |
|--------|---------|---------|
| M2 | 安装、健康检查、网络配置 | 文件布局、systemd 状态、API 连通 |
| M2.5 | mirror sync 到本地、mirror serve 文件下载、mirror verify 完整性 | manifest 正确性、制品 sha256 一致、HTTP 端点可达 |
| M3 | 备份创建/恢复、升级/回滚、配置导出 | 备份完整性、回滚后服务恢复、导出文件正确性 |
| M4 | 全场景回归、退出码一致性、非 TTY 兼容 | 退出码表、`--non-interactive` 无 hang |
