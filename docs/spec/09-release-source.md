# Release Source 与镜像管理

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 概述

Landscape Kit 需要从远程源获取 Landscape release 制品（二进制、静态资源等）。为解决中国区域 GitHub 访问不稳定的问题，以及支持内网/私有部署场景，设计统一的多源下载架构与镜像管理工具。

核心设计决策：

- **多源并发探测**：同时 HEAD 所有候选源，选延迟最低的
- **三级优先级**：CLI 显式指定 > `lkit.toml` 配置 > 内置默认
- **Trait 抽象**：`ReleaseSource` trait 统一所有源类型，可测试、可扩展
- **内置镜像工具**：`lkit mirror` 子命令，负责从上游拉取、生成 manifest、推送到目标存储
- **HTTP GET 统一下载**：终端用户下载仅需 reqwest，无需 S3 SDK

## 3. Release Source 多源模型

### 3.1 ReleaseSource Trait

在 `lkit-core` 中定义 `ReleaseSource` trait，所有源类型实现此 trait：

```rust
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// 源的显示名称，用于日志和诊断
    fn name(&self) -> &str;

    /// 列出可用版本
    async fn list_versions(&self) -> Result<Vec<String>, SourceError>;

    /// 获取指定版本的制品列表
    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError>;

    /// 获取单个制品的下载 URL
    fn artifact_url(&self, tag: &str, name: &str) -> String;

    /// 健康检查（HEAD 请求），返回延迟
    async fn probe(&self, tag: &str) -> Result<Duration, SourceError>;
}
```

### 3.2 错误类型

`SourceError` 定义在 `lkit-core`（使用 `thiserror`）：

```rust
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("IO 错误: {0}")]
    Io(String),
    #[error("版本 {tag} 不存在")]
    VersionNotFound { tag: String },
    #[error("制品 {name} 不存在")]
    ArtifactNotFound { name: String },
    #[error("manifest 解析失败: {0}")]
    InvalidManifest(String),
    #[error("源探测超时")]
    ProbeTimeout,
}
```

`SourceError` 使用 String 而非具体 IO 类型，保持 `lkit-core` 无外部 IO 依赖。`lkit-client` 实现层负责将 `reqwest::Error` / `std::io::Error` 转为 String。

`MirrorError` 定义在 `lkit-mirror` crate（使用 `thiserror`）：

```rust
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("上传失败: {0}")]
    UploadFailed(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("GitHub API 错误: {0}")]
    GitHubApi(String),
    #[error("目标存储错误: {0}")]
    TargetError(String),
}
```

### 3.3 内置源类型

| 类型 | 说明 | 实现位置 |
|------|------|---------|
| `GithubSource` | GitHub Releases API | `lkit-client` |
| `HttpMirrorSource` | 任意 HTTP(S) 镜像，含 R2 公开桶 | `lkit-client` |
| `LocalSource` | 本地目录 / `file://` 路径 | `lkit-client` |

### 3.4 源配置模型

在 `lkit.toml` 中配置源列表：

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

[[sources]]
name = "local"
type = "local"
path = "/srv/landscape-mirror/landscape"
priority = 30
```

`base_url` / `path` 指向产品目录（含产品名前缀），而非镜像根目录。`type = "github"` 自动处理路径。

字段说明：

- `name`：源的唯一标识
- `type`：`github` | `http` | `local`
- `priority`：数值越小优先级越高，同优先级的源参与并发探测
- `base_url` / `path` / `repo`：按类型不同的连接参数

### 3.5 内置默认源

未配置任何源时，使用内置默认：

```toml
# 内置默认（无需用户配置）
[[sources]]
name = "github-default"
type = "github"
repo = "ThisSeanZhang/landscape"
priority = 100
```

### 3.6 三级优先级

1. **CLI 显式指定**：`lkit install --source <url>` 或 `lkit install --source local:///path`
2. **`lkit.toml` 配置**：用户声明的 `[[sources]]` 列表
3. **内置默认**：硬编码的 GitHub Releases

CLI 显式指定的源优先级最高，会插入到探测列表首位，其余源仍作为 fallback。

### 3.7 并发探测策略

`SourceResolver`（位于 `lkit-app/src/source/resolver.rs`）负责探测与选择最优源：

```rust
/// 源探测结果
pub struct ProbeResult {
    pub source_name: String,
    pub latency: Duration,
    pub manifest: ReleaseManifest,
}

/// 多源解析器
pub struct SourceResolver {
    sources: Vec<Arc<dyn ReleaseSource>>,
}

impl SourceResolver {
    /// 并发探测所有源，选延迟最低的
    pub async fn resolve(&self, tag: Option<&str>) -> Result<ProbeResult, SourceError>;

    /// 获取选中源的制品下载 URL
    pub fn artifact_url(&self, result: &ProbeResult, artifact_name: &str) -> String;
}
```

探测流程：

1. 收集所有候选源（按优先级排序后，同优先级的合并为一个探测批次）
2. 确定探测目标：若用户指定了版本，用该版本；否则用各源的 `latest` 指针
3. 并发发送 HEAD 请求到所有源（探测目标版本的 manifest 或 checksum 文件）
4. 收集所有响应，选择延迟最低且返回 200 的源
5. 若所有源均失败，返回错误并列出每个源的失败原因

探测超时：单源 10 秒，总超时 15 秒。

### 3.8 Fallback 行为

并发探测本身已实现"选最优"。当最优源在实际下载过程中失败时：

1. 尝试下一个延迟最低的源
2. 重试最多 2 次（不同源）
3. 全部失败则报错

## 4. Release Manifest Schema

### 4.1 概述

`release-manifest.json` 是标准化的 release 制品清单，由 `lkit mirror sync` 自动生成。lkit 客户端优先读取 manifest 获取制品元数据，没有 manifest 时 fallback 到 `SHASUM256sum.txt`。

### 4.2 Schema

```json
{
  "format_version": 1,
  "tag": "v0.19.2",
  "generated_at": "2026-05-29T12:00:00Z",
  "generated_by": "lkit 0.1.0",
  "artifacts": [
    {
      "name": "landscape-webserver-x86_64",
      "sha256": "a1b2c3d4e5f6...",
      "size": 128669136,
      "arch": "x86_64"
    },
    {
      "name": "landscape-webserver-aarch64",
      "sha256": "f6e5d4c3b2a1...",
      "size": 118326864,
      "arch": "aarch64"
    },
    {
      "name": "redirect_pkg_handler-x86_64",
      "sha256": "1a2b3c4d5e6f...",
      "size": 5274240,
      "arch": "x86_64"
    },
    {
      "name": "static.zip",
      "sha256": "6f5e4d3c2b1a...",
      "size": 2094841,
      "arch": null
    }
  ]
}
```

字段说明：

| 字段 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `format_version` | int | 是 | manifest 格式版本，当前为 1 |
| `tag` | string | 是 | release 版本标签 |
| `generated_at` | ISO 8601 | 是 | 生成时间 |
| `generated_by` | string | 否 | 生成工具版本 |
| `artifacts` | array | 是 | 制品列表 |
| `artifacts[].name` | string | 是 | 文件名 |
| `artifacts[].sha256` | string | 是 | SHA-256 校验和（hex） |
| `artifacts[].size` | int | 是 | 文件大小（字节） |
| `artifacts[].arch` | string \| null | 否 | 架构标签，非架构相关制品为 null |

### 4.3 Manifest 发现规则

按优先级尝试：

1. `<base_url>/<prefix>/<tag>/release-manifest.json`（标准位置）
2. `<base_url>/<prefix>/<tag>/SHASUM256sum.txt`（fallback，上游原始校验文件）

其中 `<prefix>` 为产品目录（如 `landscape`、`landscape-kit`），由 `--prefix` 参数或 repo 名自动推导。

使用 manifest 时：直接从 manifest 读取制品列表和校验和。
使用 SHASUM256sum.txt 时：解析校验和，制品列表按文件名约定推断。

### 4.4 Latest 指针

每个产品的每个源维护一个 `latest` 指针，指向最新版本的 tag：

- HTTP 源：`<base_url>/<prefix>/latest` 文件，内容为纯文本 tag（如 `v0.19.2`）
- GitHub 源：通过 GitHub API 获取 latest release
- 本地源：`<path>/<prefix>/latest` 文件

## 5. 镜像目录规范

任何第三方均可按此规范搭建兼容镜像。lkit 客户端可从任何符合规范的镜像下载制品。

### 5.1 目录结构

所有产品统一使用 `<产品目录>/<版本>/` 结构，根目录不堆放版本文件夹：

```
<base_url>/
  landscape/                              # landscape 产品
    latest                                # 纯文本，内容为最新 tag
    v0.19.2/
      release-manifest.json               # 制品清单（推荐）
      SHASUM256sum.txt                    # 校验和文件（可选，与上游兼容）
      landscape-webserver-x86_64
      landscape-webserver-aarch64
      landscape-webserver-loongarch64
      landscape-webserver-riscv64
      landscape-webserver-s390x
      landscape-webserver-x86_64-musl
      redirect_pkg_handler-x86_64
      redirect_pkg_handler-aarch64
      redirect_pkg_handler-loongarch64
      redirect_pkg_handler-riscv64
      redirect_pkg_handler-s390x
      redirect_pkg_handler-x86_64-musl
      static.zip
    v0.19.1/
      ...
  landscape-kit/                          # lkit 产品
    latest
    v0.1.0/
      release-manifest.json
      landscape-kit-x86_64-linux
      landscape-kit-aarch64-linux
      ...
```

产品目录名默认从 repo 名推导（去掉 owner），例如：
- `ThisSeanZhang/landscape` → `landscape/`
- `landscape-router/landscape-kit` → `landscape-kit/`

可通过 `--prefix` 自定义覆盖。

### 5.2 命名约定

- 产品目录名从 repo 名自动推导（去掉 owner），可用 `--prefix` 覆盖
- Tag 目录名与 GitHub Releases tag 一致（含 `v` 前缀）
- 制品文件名与上游 GitHub Releases 一致
- `release-manifest.json` 由 `lkit mirror sync` 生成，上游不提供

### 5.3 搭建私有镜像

最简步骤：

```bash
# 1. 从 GitHub 同步到本地目录（自动放到 landscape/ 子目录）
lkit mirror sync --target local --path /srv/landscape-mirror

# 2a. 方式一：直接起 HTTP 服务
lkit mirror serve --path /srv/landscape-mirror --port 8080

# 2b. 方式二：用 Nginx/Caddy 等托管目录
# 将 /srv/landscape-mirror 作为 web root 即可

# 3. 客户端配置
# 在 lkit.toml 中添加源指向镜像地址
# base_url = "http://mirror-host:8080/landscape"
```

## 6. lkit-mirror 工具

### 6.1 定位

`lkit mirror` 是内置的镜像管理子命令，职责：

- 从上游 GitHub Releases 拉取 release 制品
- 生成标准化的 `release-manifest.json`
- 推送到目标存储（S3/R2/本地目录）
- 提供轻量 HTTP 服务

### 6.2 Crate 结构

```
crates/
  lkit-mirror/     # lib crate：镜像逻辑（sync/serve/verify/list）
  lkit-cli/        # binary crate：产出 lkit，内置 mirror 子命令
```

`lkit-mirror` 是 **lib crate**，`lkit-cli` 依赖它获得 `lkit mirror` 子命令能力。对外只发布一个二进制 `lkit`。

> 实现前提：需将 `crates/lkit-mirror` 加入 workspace `Cargo.toml` 的 `members` 数组。

### 6.3 依赖关系

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

`lkit-mirror` 是 lib crate，不依赖 `lkit-app` / `lkit-client`，仅依赖 `lkit-core`。

### 6.4 发布策略

只发布一个二进制 `lkit`，镜像管理功能内置：

- S3 SDK（aws-sdk-s3）直接打包进 `lkit`
- `lkit mirror sync/serve/verify/list` 作为内置子命令
- 不需要 feature flag，不需要单独发布 `lkit-mirror` 二进制

### 6.5 命令设计

```
lkit mirror sync [OPTIONS]       # 从上游同步 release
lkit mirror serve [OPTIONS]      # 启动 HTTP 服务
lkit mirror verify [OPTIONS]     # 校验目标完整性
lkit mirror list [OPTIONS]       # 列出已同步版本
```

#### sync

从 GitHub Releases 拉取制品，生成 manifest，推送到目标。

**来源参数**：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--repo` | `ThisSeanZhang/landscape` | GitHub 仓库（`owner/repo`） |
| `--prefix` | repo 名（如 `landscape`） | 目标产品目录，自动从 repo 名推导，MUST NOT 包含尾斜杠 |

通过 `--repo` 可以同步任意产品的 release。例如同步 lkit 自身的 release：

```bash
lkit mirror sync --repo landscape-router/landscape-kit --target s3 --bucket my-mirror
```

`--prefix` 默认从 repo 名推导（`landscape`、`landscape-kit`），无需手动指定。可自定义覆盖：

```bash
# 自定义目录名
lkit mirror sync --repo landscape-router/landscape-kit --prefix lkit \
  --target s3 --bucket my-mirror
```

**范围控制**（互斥参数）：

| 参数 | 语义 | 典型场景 |
|------|------|---------|
| （默认） | 只同步 latest | CI webhook 触发增量更新 |
| `--tag <tag>` | 单个版本 | 指定回退/补同步 |
| `--latest <N>` | 最近 N 个版本 | 常用版本镜像 |
| `--since <tag>` | 某版本之后的所有版本 | 增量补全 |
| `--all` | 全部历史版本 | 首次建镜像 |

`--since` 的"之后"按 GitHub API 返回的发布时间（`published_at`）排序，即 GitHub release 列表的自然顺序。

**增量同步**：默认跳过目标中已存在的版本（以 `<tag>/release-manifest.json` 存在且所有制品文件完整为准）。`--force` 强制重新同步覆盖已有版本。

```bash
# 同步最新 release 到本地目录（默认行为）
lkit mirror sync --target local --path /srv/mirror

# 同步指定版本到 R2
lkit mirror sync --target s3 --bucket my-mirror \
  --endpoint https://<account>.r2.cloudflarestorage.com \
  --tag v0.19.2

# 首次建镜像：同步所有历史版本
lkit mirror sync --target local --path /srv/mirror --all

# 同步最近 5 个版本
lkit mirror sync --target local --path /srv/mirror --latest 5

# 补同步某个版本之后的所有版本
lkit mirror sync --target local --path /srv/mirror --since v0.18.0

# 强制重新同步（覆盖已有版本）
lkit mirror sync --target local --path /srv/mirror --tag v0.19.2 --force
```

sync 流程：

1. 根据范围参数，调用 GitHub API 获取候选 release 列表
2. 检查目标中已存在的版本，跳过完整的（除非 `--force`）
3. 下载缺失版本的所有 artifacts 到临时目录
4. 计算每个文件的 sha256
5. 生成 `release-manifest.json`
6. 上传所有文件到目标（保留目录结构）
7. 更新 `latest` 指针
8. 清理临时目录

#### serve

启动轻量 HTTP 文件服务，将本地镜像目录暴露为 HTTP 端点。

```bash
# 默认监听 0.0.0.0:8080
lkit mirror serve --path /srv/mirror

# 自定义端口和绑定地址
lkit mirror serve --path /srv/mirror --port 9090 --bind 127.0.0.1
```

服务端点：

| 路径 | 说明 |
|------|------|
| `GET /<prefix>/latest` | 返回指定产品的最新 tag |
| `GET /<prefix>/<tag>/release-manifest.json` | 返回指定制品版本 manifest |
| `GET /<prefix>/<tag>/<artifact>` | 下载指定制品 |

其中 `<prefix>` 为产品目录名（如 `landscape`、`landscape-kit`）。

serve 不提供认证、HTTPS、限流等生产级功能，面向内网快速部署场景。生产环境建议用 Nginx/Caddy 托管目录。

#### verify

校验目标存储中的制品完整性。

```bash
lkit mirror verify --target local --path /srv/mirror
lkit mirror verify --target s3 --bucket my-mirror --endpoint https://...
```

校验内容：

- 每个版本目录下存在 `release-manifest.json`
- manifest 中声明的每个制品文件存在
- 每个制品的 sha256 与 manifest 一致
- `latest` 指向的版本存在

#### list

列出目标中已同步的版本。

```bash
lkit mirror list --target local --path /srv/mirror
lkit mirror list --target s3 --bucket my-mirror --endpoint https://...
```

### 6.6 Target Trait

`lkit-mirror` crate 中定义 `MirrorTarget` trait：

```rust
#[async_trait]
pub trait MirrorTarget: Send + Sync {
    /// 上传文件
    async fn upload(&self, key: &str, data: &[u8]) -> Result<(), MirrorError>;

    /// 检查文件是否存在
    async fn exists(&self, key: &str) -> Result<bool, MirrorError>;

    /// 读取文件内容
    async fn read(&self, key: &str) -> Result<Vec<u8>, MirrorError>;

    /// 列出指定前缀下的文件
    async fn list(&self, prefix: &str) -> Result<Vec<String>, MirrorError>;

    /// 删除文件
    async fn delete(&self, key: &str) -> Result<(), MirrorError>;
}
```

### 6.7 内置 Target 实现

| Target | 说明 | 依赖 |
|--------|------|------|
| `LocalTarget` | 本地文件系统 | std::fs（tokio 封装） |
| `S3Target` | S3/R2 兼容存储 | `aws-sdk-s3` |

`S3Target` 通过 `--endpoint` 参数支持任意 S3 兼容存储（Cloudflare R2、MinIO、AWS S3 等）。

## 7. GitHub 上游交互

### 7.1 现状

Landscape 上游在 GitHub Releases 发布制品，结构为扁平文件列表，无 manifest 文件。有 `SHASUM256sum.txt` 提供校验和。

### 7.2 策略

- `lkit mirror sync` 通过 GitHub API 获取 release 信息，自动补充生成 `release-manifest.json`
- lkit 客户端下载时，优先读 manifest，fallback 到 `SHASUM256sum.txt`
- **不依赖上游改动**，上游完全无感
- 长期推动上游添加 manifest，届时 lkit 可直接使用

### 7.3 GitHub API 使用

- 使用 GitHub REST API `GET /repos/{owner}/{repo}/releases`
- 未认证请求限制 60 次/小时，足够 `lkit mirror sync` 使用
- 可选 `GITHUB_TOKEN` 环境变量提升限额至 5000 次/小时

## 8. 对 lkit install 的影响

### 8.1 下载流程变更

原流程（spec 03-lifecycle 6.7 安装状态机步骤 3-4）的 "Resolve Release" + "Fetch Artifacts" 步骤更新为：

1. **收集候选源**：CLI 指定 > `lkit.toml` 配置 > 内置默认
2. **并发探测**：HEAD 所有源，选延迟最低的
3. **获取 manifest**：从选定源获取 `release-manifest.json`，fallback 到 `SHASUM256sum.txt`
4. **确定目标制品**：根据当前架构从 manifest 中筛选
5. **下载**：从选定源下载制品
6. **校验**：sha256 校验，不通过则尝试下一个源

### 8.2 依赖

`lkit install` 的下载功能仅依赖 `reqwest`（已有依赖），不需要新增 crate 依赖。S3 SDK 仅在 `lkit-mirror` 中使用。

## 9. 下载策略

### 9.1 设计目标

Landscape 二进制约 120MB，`static.zip` 约 2MB。需要在不同网络环境下（GitHub 直连、R2 镜像、内网）提供合理的下载速度。

### 9.2 下载架构

遵循 DI 模式（与 `LkitClient`、`ServiceManager`、`LogReader` 一致）：

- **`ArtifactDownloader` trait** 定义在 `lkit-core`，`lkit-app` 通过 `Arc<dyn ArtifactDownloader>` 注入
- **`HttpDownloader` 实现** 在 `lkit-client`，使用 `reqwest`
- `lkit-app` 不直接依赖 `reqwest`

```rust
// lkit-core：trait 定义 + 配置模型
/// 下载配置
pub struct DownloadConfig {
    /// 文件间并行数
    pub concurrent_files: usize,   // 默认 4
    /// 单文件分块数（1 = 不分块）
    pub chunks_per_file: usize,    // 默认 1
}

/// 下载进度回调
pub trait DownloadProgress: Send + Sync {
    fn on_file_start(&self, name: &str, total_bytes: u64);
    fn on_file_progress(&self, name: &str, bytes_downloaded: u64);
    fn on_file_complete(&self, name: &str);
}

/// 下载错误（lkit-core，使用 thiserror）
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("IO 错误: {0}")]
    Io(String),
    #[error("校验不匹配: 期望 {expected}, 实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("下载不完整: {downloaded}/{total} 字节")]
    Incomplete { downloaded: u64, total: u64 },
}

/// 制品下载 trait
#[async_trait]
pub trait ArtifactDownloader: Send + Sync {
    /// 下载单个文件到目标路径
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        config: &DownloadConfig,
        progress: Option<&dyn DownloadProgress>,
    ) -> Result<(), DownloadError>;
}

// lkit-client：实现
pub struct HttpDownloader {
    client: reqwest::Client,
}
```

URL 正确性由调用方（`SourceResolver`）保证。V1 不在类型层面强制 source-downloader 配对。

### 9.3 并行模型

采用两层并行策略：

**文件级并行**（默认启用）：多个制品同时下载。选定源后，并发下载 binary、static.zip、redirect_pkg_handler 等文件。

**单文件分块下载**（默认关闭，可选启用）：单个大文件通过 HTTP Range 请求拆分为多段并行下载。

默认配置按场景区分（见 9.5），`lkit.toml` 的 `[download]` 段可覆盖。

### 9.4 分块下载策略

经调研，GitHub Releases 和 R2 均支持 HTTP Range 请求（返回 206 Partial Content），但存在限制：

| 源 | Range 支持 | 已知风险 |
|---|---|---|
| GitHub Releases | 完整（Azure Blob + Varnish CDN） | `browser_download_url` 限流规则不透明，并行连接可触发 403 |
| R2 公开桶 | 支持，但首次请求可能返回 200 全量 | Cloudflare CDN 缓存干扰 >512MB 文件（landscape 二进制 ~120MB 不受影响） |
| 本地文件系统 | 不适用 | 直接文件 IO |

**策略**：分块下载作为可选优化，默认关闭。通过 `ArtifactDownloader` trait（见 9.2）实现，支持运行时 fallback：

```rust
// HttpDownloader 实现（lkit-client），progress 参数省略，完整签名见 9.2
async fn download(&self, url: &str, dest: &Path, config: &DownloadConfig, ...) -> Result<(), DownloadError> {
    // 1. HEAD 获取 Content-Length 和 Accept-Ranges
    // 2. 若 chunks > 1 且 Accept-Ranges: bytes → 分块并行下载
    // 3. 若服务端不支持 Range（返回 200）→ fallback 到单连接
    // 4. 任一分块失败 → fallback 到单连接重试
}
```

### 9.5 不同场景的默认配置

| 场景 | 文件并行 | 单文件分块 | 说明 |
|------|---------|-----------|------|
| `lkit install` | 4 | 1（关闭） | 终端用户默认安全配置 |
| `lkit mirror sync` | 4 | 4 | 运维工具，可控环境下启用分块 |
| 用户自定义 | 可配置 | 可配置 | 通过 `lkit.toml` 或 CLI 参数覆盖 |

### 9.6 进度展示

下载过程中通过 `indicatif` 展示：

- 总进度条：所有制品的整体进度
- 单文件进度条：每个文件的下载进度
- 速度与 ETA：基于最近 N 秒的平均速度计算

## 10. 里程碑调整

本设计引入新的里程碑 M2.5，原有 M2 和 M3 不变：

### M2：安装与基础设施（不变）

- 安装状态机、systemd 安装、引导式网络配置
- 单源下载（GitHub Releases）
- `SHASUM256sum.txt` 校验

### M2.5：多源下载与镜像工具

- `ReleaseSource` trait 与多源并发探测
- `release-manifest.json` schema 实现
- `Downloader` 抽象：文件级并行 + 可选单文件分块
- `lkit-mirror` lib crate + `lkit mirror` 内置子命令
- S3/R2 target 实现
- Local target 实现
- `lkit mirror serve` HTTP 服务
- `lkit mirror sync` 范围控制（`--tag` / `--latest N` / `--since` / `--all`）与增量同步
- 镜像规范文档

### M3：备份、恢复与更新回滚（不变）

- 使用 M2.5 的多源架构获取 release 制品

## 11. CI 同步（可选）

可选提供 GitHub Actions workflow 模板，用于自动同步 release 到 R2：

```yaml
# .github/workflows/mirror-sync.yml（模板）
# 同步 landscape 和 lkit 的 release 到 R2 镜像
on:
  repository_dispatch:
    types: [landscape-release, lkit-release]
  workflow_dispatch:
    inputs:
      product:
        description: 'Product to sync (landscape or lkit)'
        required: true
        type: choice
        options: [landscape, lkit]
      tag:
        description: 'Release tag to sync (empty = latest)'

jobs:
  sync:
    runs-on: ubuntu-latest
    env:
      AWS_ACCESS_KEY_ID: ${{ secrets.R2_ACCESS_KEY }}
      AWS_SECRET_ACCESS_KEY: ${{ secrets.R2_SECRET_KEY }}
    steps:
      - uses: actions/checkout@v4
      - name: Install lkit
        run: cargo install --path crates/lkit-cli
      - name: Sync landscape
        if: github.event_name == 'repository_dispatch' && github.event.action == 'landscape-release'
           || github.event.inputs.product == 'landscape'
        run: >
          lkit mirror sync
          --target s3 --bucket ${{ vars.R2_BUCKET }} --endpoint ${{ vars.R2_ENDPOINT }}
          ${{ inputs.tag && format('--tag {0}', inputs.tag) || '' }}
      - name: Sync lkit
        if: github.event_name == 'repository_dispatch' && github.event.action == 'lkit-release'
           || github.event.inputs.product == 'lkit'
        run: >
          lkit mirror sync
          --repo landscape-router/landscape-kit --prefix landscape-kit
          --target s3 --bucket ${{ vars.R2_BUCKET }} --endpoint ${{ vars.R2_ENDPOINT }}
          ${{ inputs.tag && format('--tag {0}', inputs.tag) || '' }}
```

同一套 sync 代码，通过 `--repo` 和 `--prefix` 区分产品。landscape-kit 的 CI 发布自身 release 时，触发 `repository_dispatch` 同步 lkit 到 R2。

此 workflow 为参考模板，用户根据自身环境调整。
