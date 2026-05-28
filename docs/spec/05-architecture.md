# 技术架构与代码结构

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 技术栈

### 2.1 核心栈

- Rust
- `clap`：CLI 参数解析
- `dialoguer`：引导式交互（输入、选择、确认）
- `console`：终端格式化输出
- `indicatif`：进度条与 spinner
- `comfy-table`：表格输出
- `tokio`：异步运行时
- `reqwest`：调用 Landscape API / 下载 release 制品
- `serde` / `serde_json` / `toml`：序列化与配置解析
- `tracing` / `tracing-subscriber`：日志
- `anyhow`：CLI 层最终错误展示
- `thiserror`：库层错误类型定义（`lkit-core`、`lkit-client`、`lkit-app`、`lkit-mirror`）

### 2.2 辅助栈

- `sha2`：校验
- `flate2` + `tar`：备份归档

### 2.3 镜像工具栈（`lkit-mirror`）

- `aws-sdk-s3`：S3/R2 兼容存储上传
- `axum`：轻量 HTTP 服务（`lkit mirror serve`），由 `lkit-cli` 传入 tokio runtime handle

### 2.4 后续可扩展

- 若需要 `htop` 式实时交互面板，可引入 `ratatui` + `crossterm`，`lkit-app` 层用例代码无需变更
- `--watch` 模式的表格刷新（定时清屏重绘）在当前栈内可直接实现

## 3. 代码结构

使用独立 Rust workspace，拆分为以下逻辑层：

```text
crates/
  lkit-core       # 公共模型、配置、错误、trait 定义（ServiceManager, LogReader, LkitClient, ReleaseSource）
  lkit-client     # 外部 IO 实现层：API 客户端、systemd 管理、日志文件读取、ReleaseSource 实现
  lkit-app        # 用例层：backup/restore/upgrade/export/status/service/logs/diagnose + SourceResolver
  lkit-mirror     # 镜像管理 lib crate（sync/serve/verify/list 逻辑，lkit-cli 依赖）
  lkit-cli        # clap 命令入口 + 引导式交互 + i18n 消息（产出二进制：lkit）
```

### 3.1 依赖关系

```
lkit-cli ──→ lkit-app ──→ lkit-client ──→ lkit-core
   │              │                              ↑
   └──────────────┘ (lkit-cli 直接依赖 lkit-core) │
   │                                               │
   └──→ lkit-mirror (lib) ─────────────────────────┘
            │
            ├── reqwest
            └── aws-sdk-s3
```

- `lkit-mirror` 是 lib crate，不依赖 `lkit-app` / `lkit-client`
- `lkit-cli` 依赖 `lkit-mirror`，内置 `lkit mirror` 子命令
- 只发布一个二进制 `lkit`，S3 SDK 直接打包

- tokio runtime 在 `lkit-cli` 的 `main()` 中启动
- `lkit-client` 实现注入到 `lkit-app` 用例中（trait 抽象，方便测试 mock）
- `lkit-app` 不依赖 `clap`、`dialoguer`、`console` — 只依赖数据结构和 async trait
- `lkit self version` 不需要 Landscape HOME 存在，可在任意环境执行
- `LANDSCAPE_HOME` 环境变量可覆盖默认 Landscape 安装路径（默认 `~/.landscape-router`）

### 3.2 分层原则

- 所有核心业务逻辑收敛在 `lkit-app`
- `lkit-client` 负责所有外部 IO 实现：Landscape API HTTP 调用、systemd shell 管理（`ServiceManager` trait）、日志文件读取（`LogReader` trait）
- `lkit-core` 定义共享协议（`LkitClient`、`ServiceManager`、`LogReader` trait）、模型、错误类型
- `lkit-cli` 负责 CLI 参数解析与引导式交互，不实现业务逻辑

### 3.3 核心 Trait 抽象

`lkit-core` 定义三个 async trait 用于依赖注入：

| Trait | 用途 | 定义位置 | 实现位置 |
|-------|------|---------|---------|
| `LkitClient` | Landscape API 调用 | `lkit-core` | `lkit-client`（`LandscapeClient`） |
| `ServiceManager` | systemd 服务管理 | `lkit-core` | `lkit-client`（`SystemdManager`） |
| `LogReader` | 日志文件读取 | `lkit-core` | `lkit-client`（`FileLogReader`） |
| `ReleaseSource` | release 源抽象 | `lkit-core` | `lkit-client`（`GithubSource` / `HttpMirrorSource` / `LocalSource`） |
| `ArtifactDownloader` | 制品下载 | `lkit-core` | `lkit-client`（`HttpDownloader`） |
| `MirrorTarget` | 镜像目标存储 | `lkit-mirror` | `lkit-mirror`（`S3Target` / `LocalTarget`） |

trait 定义在 `lkit-core`（消费者），实现在 `lkit-client`（生产者），在 `lkit-cli` 的 `main()` 中组装注入。测试时用 mock 实现替代。

### 3.4 lkit-app 模块概览

```
lkit-app/
  install/       # InstallUseCase（安装状态机）
  backup/        # BackupUseCase, RestoreUseCase
  upgrade/       # UpgradeUseCase, RollbackUseCase
  status/        # StatusUseCase — systemd + API 状态查询
  service/       # ServiceUseCase — systemd 服务启停控制
  logs/          # LogsUseCase — 日志文件读取
  diagnose/      # DiagnoseUseCase — 系统健康检查
  config/        # ConfigExportUseCase
  self_upgrade/   # SelfUpgradeUseCase
  source/        # SourceResolver（多源并发探测与选择）；ArtifactDownloader trait 在 lkit-core，实现在 lkit-client
```

每个用例通过 struct 暴露，构造函数接收 `Arc<dyn Trait>` 依赖注入，用例方法返回 `Result<T, AppError>`。

## 4. 错误处理

| 层 | 策略 |
|---|---|
| `lkit-core` | 使用 `thiserror` 定义 `CoreError` 枚举 |
| `lkit-client` | 使用 `thiserror` 定义 `ClientError` 枚举（网络错误、API 错误、序列化错误） |
| `lkit-app` | 使用 `thiserror` 定义 `AppError` 枚举，包含内层错误的 variant |
| `lkit-cli` | 使用 `anyhow` + `console` 做最终展示，将库错误转为用户可读信息，带操作建议 |

错误输出格式统一为：
```
Error: <简短描述>
Caused by: <底层原因>
Suggestion: <建议操作>
```

## 5. 管理器配置

### 5.1 配置文件

- 路径：`{manager_home}/config/lkit.toml`（默认 `~/.landscape-kit/config/lkit.toml`）
- 格式：TOML
- 首次运行时若不存在，使用内置默认值

### 5.2 V1 配置项

```toml
[[sources]]
name = "r2-official"
type = "http"
base_url = "https://dl.landscape.example.com/landscape"
priority = 10

[[sources]]
name = "github"
type = "github"
repo = "ThisSeanZhang/landscape"
priority = 20

[download]
concurrent_files = 4          # 文件间并行下载数
chunks_per_file = 1           # 单文件分块数（1 = 不分块）

[backup]
max_auto_backups = 5          # 自动备份保留数量
```

源配置说明：详见 [09-release-source.md](./09-release-source.md)。未配置任何源时使用内置默认（GitHub Releases）。`lkit install --source <url>` 可临时覆盖。

## 6. 管理器自身日志

- 日志输出到 stderr（不污染 stdout 的表格/状态输出）
- 日志级别：默认 `WARN`，通过 `-v` / `--verbose` 升到 `INFO`，`-vv` 升到 `DEBUG`
- 同时支持 `RUST_LOG` 环境变量覆盖
- 不写入 Landscape 的日志目录（管理器与 Landscape 日志分离）
- `lkit logs` 命令查看的是 Landscape 日志，不是管理器自身日志

## 7. 测试策略

### 7.1 单元测试

- 备份 metadata 编解码
- backup index 校验规则
- 路径规范化与逃逸防护
- 恢复计划生成

### 7.2 集成测试

- HOME 发现逻辑（含空目录、权限拒绝）
- 导出配置流程
- 备份点创建/校验/恢复（含损坏备份文件处理）
- 升级前自动创建备份点
- 恢复后健康检查
- 并发执行检测（pidfile）

### 7.3 端到端验证

- 本机安装 -> 创建备份点 -> 修改状态 -> 恢复备份点
- 导出配置 -> 新实例重建
- 升级失败 -> 自动回滚

> E2E 测试架构详见 [08-testing.md](./08-testing.md)

## 8. 退出码

| 码 | 含义 |
|----|------|
| 0 | 成功 |
| 1 | 通用错误 |
| 2 | 权限不足（需 root/sudo） |
| 3 | Landscape 未安装或 HOME 不可访问 |
| 4 | 网络/API 不可达 |
| 5 | 备份/恢复/升级操作失败 |
| 6 | 系统依赖不满足（systemd 不可用等） |

## 9. Landscape API 依赖清单

管理器依赖以下 Landscape API 端点（需 Landscape 侧支持或通过 PR 补充）：

| 端点 | 用途 | 方法 |
|---|---|---|
| `/api/v1/status` | 获取运行状态、版本信息 | GET |
| `/api/v1/health` | 健康检查（启动后校验） | GET |
| `/api/v1/config/export` | 导出当前配置为 init 格式 | GET |
| `/api/v1/system/info` | 获取系统信息（网卡列表等） | GET |

- 所有 API 调用通过 `lkit-client` 封装
- API base URL 从 `landscape.toml` 的 `api.listen` 字段读取，默认 `http://127.0.0.1:8080`
- API 不可达时降级为仅本机操作（例如仅通过 systemctl 查看进程状态）
- API 响应格式需与 Landscape 侧对齐，实现时定义具体 schema
