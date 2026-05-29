# 安装流程实现规格

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit
- 依赖：[03-lifecycle.md](./03-lifecycle.md)（安装状态机）、[09-release-source.md](./09-release-source.md)（多源探测架构）

本文件是安装流程的具体实现规格，覆盖从系统检测到启动验证的完整数据流。

## 2. 概述

```
sudo lkit install
  │
  ├─[1] 系统检测 → target = "x86_64" 或 "x86_64-musl"
  ├─[2] 加载源配置 → CLI > lkit.toml > 内置默认
  ├─[3] 并发探测 → 表格展示 → 用户选源（非交互模式跳过）
  ├─[4] 启动后台下载 + Wizard 1-7（并行）
  ├─[5] 等待下载完成 → 校验 → 解压
  ├─[6] TOML 生成 → systemd → 启动
  ├─[7] 健康检查（轮询 Web UI）
  └─[8] 安装报告
```

核心设计决策：

- **源选择前置**：Wizard 第一步是选源，选完后立即启动后台下载，用户在配置网络的同时下载在后台进行
- **下拉过滤**：通过解析制品文件名提取 arch/libc，精确匹配系统 target
- **异步下载**：`tokio::spawn` 后台执行，Wizard 完成后再展示进度

## 3. 系统类型检测

### 3.1 CPU 架构

从 `std::env::consts::ARCH` 获取编译目标架构，映射到制品命名惯例：

| consts::ARCH | 制品用名 |
|-------------|---------|
| `x86_64` | `x86_64` |
| `aarch64` | `aarch64` |
| `riscv64`（via `riscv64gc`） | `riscv64` |
| `s390x` | `s390x` |
| `loongarch64` | `loongarch64` |

若 `std::env::consts::ARCH` 不在上表中则直接报错退出。注意 `riscv64gc` 需映射为 `riscv64`。

### 3.2 libc 检测（glibc vs musl）

使用 **ELF dynamic linker 探测法**（与 `cargo-binstall` / `detect-targets` crate 同策略）：

**glibc 检测**：多路径顺序尝试，执行 `<path> --version`，stdout 含 `GLIBC` 或 `GNU libc` 则判定为 glibc。

按架构的探测路径表：

| 架构 | glibc linker 路径 |
|------|------------------|
| x86_64 | `/lib64/ld-linux-x86-64.so.2` |
| aarch64 | `/lib/ld-linux-aarch64.so.1`，`/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1`，`/usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1` |
| riscv64 | `/lib/ld-linux-riscv64-lp64d.so.1`，`/usr/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1` |
| s390x | `/lib/ld-linux-s390x.so.1`，`/usr/lib/s390x-linux-gnu/ld-linux-s390x.so.1` |
| loongarch64 | `/lib/ld-linux-loongarch64-lp64d.so.1`，`/usr/lib/loongarch64-linux-gnu/ld-linux-loongarch64-lp64d.so.1` |

**musl 检测**：执行 `<path> --version` 2>&1，exit code 非零但 stderr 含 `musl libc` 则判定为 musl。仅 x86_64 有 musl 变体，路径：

- `/lib/ld-musl-x86_64.so.1`

**探测顺序**：先查当前架构的 glibc 路径（第一个存在的路径 + stdout 含关键字），glibc 未命中再查 musl。都未命中时：已知仅 glibc 的架构（aarch64/riscv64/s390x/loongarch64）默认当 glibc 处理，x86_64 报错退出。

约束：检测函数为同步（`std::process::Command`），在 `SourceResolver::resolve()` 前调用。

### 3.3 产出 target 字符串

```
arch == "x86_64", libc == glibc  →  "x86_64"
arch == "x86_64", libc == musl   →  "x86_64-musl"
arch == "aarch64"                →  "aarch64"  （仅 glibc）
arch == "riscv64"                →  "riscv64"  （仅 glibc）
arch == "s390x"                  →  "s390x"    （仅 glibc）
arch == "loongarch64"            →  "loongarch64"（仅 glibc）
```

`SystemTarget` 结构体：

```rust
pub struct SystemTarget {
    pub arch: String,
    pub libc: LibcType,
    /// 制品匹配用字符串，如 "x86_64" 或 "x86_64-musl"
    pub target_str: String,
}

pub enum LibcType {
    Glibc,
    Musl,
}
```

## 4. 制品名解析

### 4.1 上游数据

从 `ThisSeanZhang/landscape` GitHub Releases 实际拉取确认（v0.19.2）：

```
landscape-webserver-x86_64           landscape-webserver-x86_64-musl
landscape-webserver-aarch64
landscape-webserver-loongarch64      landscape-webserver-riscv64
landscape-webserver-s390x
redirect_pkg_handler-x86_64          redirect_pkg_handler-x86_64-musl
redirect_pkg_handler-aarch64
redirect_pkg_handler-loongarch64     redirect_pkg_handler-riscv64
redirect_pkg_handler-s390x
static.zip                           SHASUM256sum.txt
```

已知架构：`x86_64` `aarch64` `loongarch64` `riscv64` `s390x`。仅 `x86_64` 有 `-musl` 变体。

### 4.2 解析规则

从文件名提取 `Option<ArchInfo>`：

```rust
pub struct ArchInfo {
    pub arch: String,       // "x86_64", "aarch64", ...
    pub musl: bool,         // true if name ends with "-musl"
}
```

解析逻辑：

1. 去掉文件扩展名（不适用当前命名，留作扩展点）
2. 若文件名以 `-musl` 结尾 → strip `-musl`，`musl = true`
3. 在剩余部分中匹配已知架构后缀（按长度降序匹配，避免 `x86_64` 被 `64` 误匹配）
4. 匹配成功 → 返回 `ArchInfo { arch, musl }`
5. 无匹配 → 返回 `None`（如 `static.zip`、`SHASUM256sum.txt`）

已知架构列表（常量，按后缀长度降序）：

```
["x86_64", "aarch64", "loongarch64", "riscv64", "s390x"]
```

### 4.3 与 target 的匹配

`parse_arch()` 产出 `ArchInfo` 后，`Artifact.arch` 字段存储带 libc 后缀的完整 target 字符串（如 `"x86_64"` 或 `"x86_64-musl"`），与 `SystemTarget.target_str` 做直接字符串比较：

```rust
fn matches_target(artifact: &Artifact, target: &SystemTarget) -> bool {
    match &artifact.arch {
        Some(a) => a == &target.target_str,  // 精确匹配
        None => true,                         // arch-independent (static.zip)
    }
}
```

`static.zip`（`arch: None`）总是匹配。`SHASUM256sum.txt` 和 `.sha256`/`.md5` 文件在下载阶段按文件名排除，不走 `matches_target`。

## 5. 源配置三级加载

### 5.1 CLI 参数

```rust
pub struct InstallArgs {
    #[arg(long)]
    pub init_file: Option<PathBuf>,

    #[arg(long)]
    pub source: Option<String>,      // 源名称，如 "r2-official"

    #[arg(long)]
    pub version: Option<String>,     // 版本 tag，如 "v0.19.2"

    #[arg(long, default_value_t = 6300)]
    pub web_port: u16,               // Web UI 端口

    #[arg(long)]
    pub force: bool,                 // 覆盖已安装实例
}
```

`--source` 接受源名称（`SourceConfig.name`），不匹配任何已知源时报错列出可用源名。

### 5.2 lkit.toml

路径：`~/.landscape-kit/lkit.toml`

```toml
[[sources]]
name = "company-mirror"
type = "http"
base_url = "https://mirror.internal.example.com/landscape"
priority = 5

[[sources]]
name = "r2-official"
type = "http"
base_url = "https://pub-1e112154ee8a4b909c204b5325aba1f3.r2.dev/landscape"
priority = 10
```

`SourceConfig` 模型已定义在 `lkit-core/src/source/config.rs`，需新增 loader：

```rust
pub fn load_lkit_toml() -> Result<Vec<SourceConfig>, ConfigError> {
    let path = manager_home().join("lkit.toml");
    if !path.exists() { return Ok(vec![]); }
    let content = std::fs::read_to_string(&path)?;
    let parsed: LkitToml = toml::from_str(&content)?;
    Ok(parsed.sources.unwrap_or_default())
}
```

只在文件存在时加载，不存在时无声返回空列表。TOML 解析失败时报错退出。

### 5.3 内置默认

```rust
pub fn default_sources() -> Vec<SourceConfig> {
    vec![
        SourceConfig {
            name: "r2-official".into(),
            source_type: SourceType::Http,
            priority: 10,
            base_url: Some("https://pub-1e112154ee8a4b909c204b5325aba1f3.r2.dev/landscape".into()),
            repo: None, path: None,
            // S3 字段为 None
        },
        SourceConfig {
            name: "github-default".into(),
            source_type: SourceType::Github,
            priority: 100,
            base_url: None,
            repo: Some("ThisSeanZhang/landscape".into()),
            path: None,
        },
    ]
}
```

### 5.4 合并策略

```
CLI --source "X" → 用名为 X 的源配置（从 lkit.toml 或内置默认查找）
                   若找到，直接替换整个源列表（单源模式，不探测）
CLI 未指定      → lkit.toml 有配置 → 用 lkit.toml 的 [[sources]]
                  lkit.toml 无配置 → 用内置默认
CLI --version   → 固定版本，不调用 latest_tag()
CLI 未指定版本  → 调用 latest_tag() 获取最新版本
```

合并在 `lkit-app` 层完成，产出最终的 `Vec<SourceConfig>` + `Option<String>` (version)。

源探测结果中若不同源返回不同 latest 版本（如 R2 有 v0.19.2 但 GitHub 有 v0.20.0），在源选择表格中展示各源的版本差异，帮助用户知情选择。

## 6. 异步下载设计

### 6.1 时序

```
[系统检测] → [源选择(表格)] → spawn 后台下载 → [Wizard 1-7] → await 下载 → [校验解压安装]
                    ↑                                              ↑
              用户选源确认后                                  Wizard 完成或用户确认安装后
              立即启动下载                                    join handle + 显示进度
```

`lkit-cli` 中维护：

```rust
struct DownloadContext {
    handle: JoinHandle<Result<Vec<PathBuf>, DownloadError>>,
    done_rx: tokio::sync::oneshot::Receiver<Result<(), DownloadError>>,
}
```

后台任务完成时通过 `oneshot` 通知结果。`JoinHandle` 用于取消（`abort()`）。Wizard 结束时 `done_rx.try_recv()` 检查完成状态：已完成则显示结果，未完成则等待并展示进度。

### 6.2 取消处理

用户按 Ctrl+C 或 Wizard 中选 "退出" 时：`handle.abort()`。`ctrlc` handler 中额外调用 abort。

临时文件写入 `ManagerPaths::tmp_dir`，下次 lkit 启动时清理。

### 6.3 进度展示

用户在 Wizard 中不看到下载进度（避免干扰交互）。Wizard 完成进入安装阶段后：

```
下载中...
  landscape-webserver-x86_64  ████████████████████ 128 MB ✓
  redirect_pkg_handler-x86_64 ████████████████████ 5.3 MB ✓
  static.zip                  ████████████████████ 2.1 MB ✓
```

如果 Wizard 完成时下载早已完成（快速源），直接跳过进度条，显示 "下载已完成 ✓"。

## 7. 下载过滤

### 7.1 匹配规则

```
system_target = "x86_64"

manifest artifacts:
  landscape-webserver-x86_64       → match (arch=x86_64, !musl) → DOWNLOAD
  landscape-webserver-x86_64-musl  → NO   (arch=x86_64, musl)  → SKIP
  landscape-webserver-aarch64      → NO   (arch=aarch64)       → SKIP
  redirect_pkg_handler-x86_64      → match                     → DOWNLOAD
  redirect_pkg_handler-aarch64     → NO                        → SKIP
  static.zip                       → match (no arch)           → DOWNLOAD
  SHASUM256sum.txt                 → NO   (excluded by name)   → SKIP
```

排除规则：

- 文件名含 `SHASUM` → 不下载
- 文件名以 `.sha256` / `.md5` 结尾 → 不下载
- 其余无 arch 信息的文件（如 `static.zip`）→ 下载

### 7.2 arch 字段填充

修复三类 source 的 `get_artifacts()`：

- **GithubSource**：对每个 asset name 调用 `parse_arch()`，填入 `Artifact.arch`
- **HttpMirrorSource**：解析 `release-manifest.json` 时已有 arch；解析 `SHASUM256sum.txt` 时对每个文件名调用 `parse_arch()`
- **LocalSource**：同 HttpMirrorSource

## 8. 安装执行

### 8.1 校验

下载完成后逐文件 SHA-256 校验。校验值来源：

1. `release-manifest.json` 中的 `artifacts[].sha256`
2. Fallback: `SHASUM256sum.txt`（若 manifest 中 sha256 为空）

校验或下载失败 → 自动 fallback 到延迟次低的源，最多试 2 个源。全部失败则报错退出，清理临时文件。

### 8.2 解压 static.zip

下载完成后，对两个 binary 设置可执行权限（`0o755`）：

- `landscape-webserver-{target}`
- `redirect_pkg_handler-{target}`

使用 `zip` crate（workspace 依赖 `zip = "2"`）解压 `static.zip` 到 `<landscape_home>/static/`。若 `static.zip` 不存在于 manifest 中则 warn 并跳过（未来上游格式变化不阻塞安装）。

```rust
let archive = std::fs::File::open(&static_zip_path)?;
let mut zip = zip::ZipArchive::new(archive)?;
zip.extract(&static_dir)?;
```

### 8.3 TOML 生成

现有 `config_gen.rs` 逻辑不变，直接复用。

### 8.4 systemd

现有 `apply()` 逻辑不变。追加一步：写入 `landscape_init.lock`。

```rust
// 5. Create lock file
let lock_path = home.join("landscape_init.lock");
self.host_installer.write_file(&lock_path, b"").await?;
```

### 8.5 启动与健康检查

`systemctl start` 后轮询 Web UI：

- URL: `http://127.0.0.1:{web_port}`
- 间隔: 3 秒
- 最多: 10 次（30 秒）；慢速硬件（如 Raspberry Pi）可适当放宽到 20 次
- 成功条件: HTTP 200
- 超时: 打印警告，提示用户手动验证 `systemctl status landscape`，退出码 0（安装本身成功，服务可能需要更长时间初始化）

### 8.6 安装报告

```
┌─ 安装完成 ────────────────────────────┐
│ HOME:     /root/.landscape-router      │
│ Web UI:   http://127.0.0.1:6300        │
│ 架构:     x86_64 (glibc)               │
│ 版本:     v0.19.2                      │
│ 源:       r2-official                  │
│ 状态:     服务已启动 ✓                 │
└────────────────────────────────────────┘
```

## 9. Wizard 交互设计

### 9.1 步骤重组

源选择从 Wizard 中独立出来，放在 `install.rs` 主流程中处理（`StepKind` 枚举不变）：

```
[install.rs] 源选择 → spawn 后台下载
[Wizard]     Step 1: WAN 网卡选择
             Step 2: LAN 网卡选择
             Step 3: WAN 接入方式
             Step 4: LAN 网关配置
             Step 5: Landscape 服务配置
             Step 6: 安装源与版本（显示已选信息，直通）
             Step 7: 确认安装
```

源选择的结果通过 `CollectedConfig.source_name` 和 `CollectedConfig.version` 传入 Wizard。Wizard 的 `StepKind` 枚举保持现有的 7 个变体，`Source` 步骤的 `render()` 改为只读展示（不交互）。

### 9.2 Step 0：源选择

```
正在探测可用源...

┌─ 可用源 ───────────────────────────────┐
│ 来源           │ 版本     │ 延迟   │ 状态 │
│ r2-official    │ v0.19.2  │  45ms │ ✓   │
│ github-default │ v0.20.0  │ 230ms │ ✓   │
├────────────────────────────────────────┤
│ 当前选择: r2-official                  │
│ 版本: v0.19.2                          │
└────────────────────────────────────────┘

选择源 [默认: 1, Enter 确认]
  1. r2-official
  2. github-default

> 1

✓ 已选定源 r2-official，版本 v0.19.2
  已在后台开始下载 (3 个文件)
```

实现要点：

- 并发探测全部候选源，结果按延迟排序
- 用 `comfy_table` 渲染表格（`println!` 输出），然后 `dialoguer::Select` 在下方独立渲染选项列表
- 用户用数字选择，默认选第 1 个（最快源）
- 若不同源返回不同 latest 版本，表格中直接展示差异
- 确认后立即 spawn 后台下载，然后进入 Wizard Step 1

### 9.3 每步小表格

每步完成后显示累积状态表格（`comfy_table`，紧凑样式）。

Step 1 完成后：

```
┌─ 网络 ────────────────────────────────┐
│ WAN NIC:  eth0 (aa:bb:cc:dd:ee:ff)    │
└───────────────────────────────────────┘
```

Step 3 完成后：

```
┌─ 网络 ────────────────────────────────┐
│ WAN:       eth0 → DHCP               │
│ LAN NICs:  无                        │
└───────────────────────────────────────┘
```

Step 4 完成后（多网卡模式）：

```
┌─ 网络 ────────────────────────────────┐
│ WAN:       eth0 → DHCP               │
│ LAN:       eth1 → br_lan             │
│ 网关:      192.168.5.1/24            │
└───────────────────────────────────────┘
```

Step 5 完成后：

```
┌─ 网络 ────────────────────────────────┐
│ WAN:       eth0 → DHCP               │
│ LAN:       eth1 → br_lan             │
│ 网关:      192.168.5.1/24            │
├─ 服务 ────────────────────────────────┤
│ Web 端口:  6300                       │
│ 管理员:    root                       │
└───────────────────────────────────────┘
```

### 9.4 Step 6：版本确认

源已在 Step 0 选定，版本已确定，此步骤显示信息后自动前进。

Step 7 (Summary) 保持现有结构，增加系统架构行和待下载文件列表。

## 10. CLI 参数全貌

```
lkit install
    [--init-file <path>]    # 非交互：直接用现有 TOML，跳过 Wizard + 下载
    [--source <name>]       # 指定源名称
    [--version <tag>]       # 指定版本 tag
    [--web-port <port>]     # Web UI 端口（非 Wizard 模式必需，默认 6300）
    [--force]               # 覆盖已安装实例
```

### 10.1 模式矩阵

| 参数组合 | 行为 |
|---------|------|
| 无参数 (TTY) | 系统检测 → 源选择 → Wizard → 下载 → 安装 |
| 无参数 (非 TTY) | 报错，要求指定 `--init-file` 或 `--source` + `--version` |
| `--init-file <path>` | 跳过一切，直接写 TOML + systemd（当前已实现，端口从 TOML 读取） |
| `--source X --version Y` (TTY) | 固定源/版本，跳过源选择，其余 Wizard 步骤正常 |
| `--source X --version Y` (非 TTY) | 跳 Wizard，web_port 取 `--web-port`（默认 6300），直接下载 → 安装 |
| `--force` + 任意模式 | 跳过锁检查，覆盖 HOME 目录 |

### 10.2 非 TTY 检测

```rust
fn is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) != 0 }
}
```

## 11. 错误处理矩阵

| 阶段 | 错误类型 | 处理 |
|------|---------|------|
| 系统检测 | 未知架构 / libc 探测失败 | 报错退出。不落地任何文件 |
| 源配置加载 | lkit.toml 解析失败 | 报错退出，提示检查语法 |
| 源探测 | 全部源失败 | 报错退出，列出每个源的失败原因 |
| 后台下载 | 网络错误 / checksum 不匹配 | 报错，清理 tmp 文件。不修改 HOME |
| 解压 | zip 损坏 | 报错，清理 tmp + 已下载文件 |
| TOML 写入 | IO 错误 | 报错，尝试删除已写入文件 |
| systemd | 写入 / daemon-reload / enable 失败 | 报错，不删除已写入文件（管理员手动处理） |
| 启动 | start 失败 | 报错，不撤回 |
| 健康检查 | 超时 | 警告，不报错。提示用户手动验证 |
| 锁检查 | 已安装 | 拒绝，提示 `--force` |

## 12. 代码分层

```
lkit-core
  src/system_detect.rs      # SystemTarget + detect() 函数（同步）
  src/source/name_parser.rs # parse_arch() 纯函数 + 单元测试
  src/source/config.rs      # SourceConfig 加 S3 变体 + S3 字段
  src/source/manifest.rs    # Artifact.arch 不变，matches_target 可放这里或 name_parser

lkit-client
  src/source/github.rs      # get_artifacts 调用 parse_arch 填充 arch
  src/source/http_mirror.rs # parse_shasum_file 调用 parse_arch 填充 arch
  src/source/local.rs       # get_artifacts 调用 parse_arch 填充 arch
  src/source/s3.rs          # 无改动，S3 source 读 manifest 时已有 arch

lkit-app
  src/source/build.rs       # build_one 加 S3 匹配臂
  src/source/config_loader.rs  # 新增：读 lkit.toml，合并源列表
  src/install/mod.rs        # apply() 加 lock 文件创建
  src/install/config_gen.rs # 无改动

lkit-cli
  src/cli.rs                # InstallArgs 加 --source --version --web-port --force
  src/commands/install.rs   # 主流程重构：检测→加载→探测→下载→Wizard→安装
  src/wizard/mod.rs         # build_config 取 collected.source_name
  src/wizard/steps/source.rs # 重写：源选择表格（Step 0）
  src/wizard/steps/*.rs     # 每步后加小表格
  src/progress.rs           # 下载进度展示
```

## 13. 测试策略

| 层 | 测试内容 |
|----|---------|
| `name_parser` | 已知 15 种文件名解析，边界（空字符串、无架构、未知架构） |
| `system_detect` | Mock Command 输出，覆盖 glibc/musl/无 libc 三种路径 |
| `config_loader` | TOML 解析、缺少文件、空 sources、S3 字段 roundtrip |
| `matches_target` | 精确匹配、musl vs glibc 区分、arch-independent 文件 |
| `install flow` | 现有 mock HostInstaller 测试保持，追加 lock 文件写入验证 |
