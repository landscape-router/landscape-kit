# lkit Test Fixture

## 目标

`lkit-test-fixture` 为 `lkit` 提供一个轻量、原生、可离线运行的 Landscape 测试替身，使单元测试和集成测试不需要下载或启动完整 Landscape。

fixture 只实现 `lkit` 明确依赖的稳定协议，不模拟 Landscape 的完整业务能力、数据库内容、认证、eBPF、真实 DNS 解析或 Web UI。

主要目标：

- 使用真实可执行文件完成下载、摘要校验、安装和进程身份检查；
- 在普通用户环境使用临时目录和高位端口运行；
- 模拟 Landscape 初始化文件和健康检查行为；
- 模拟配置导出 API，为 switch、backup 和 rollback 测试提供输入；
- 通过声明式故障场景覆盖启动失败、健康失败和稳定期退出；
- 使用 fake systemctl 测试 lkit 的 systemd 调用逻辑；
- 在普通用户、Docker fake-systemctl 测试和 systemd-nspawn 兼容性 smoke 中复用同一个 fake Landscape。

## 目录结构

```text
crates/lkit-test-fixture/
├── Cargo.toml
└── src/
    ├── lib.rs
    └── bin/
        ├── landscape-webserver.rs
        ├── lkit-fixture-release.rs
        └── lkit-test-systemctl.rs
```

- `src/lib.rs`：稳定契约、配置结构、API 响应类型和共享常量；
- `src/bin/landscape-webserver.rs`：原生 fake Landscape 进程；
- `src/bin/lkit-fixture-release.rs`：生成可由 `lkit-publish` 发布的测试资产；
- `src/bin/lkit-test-systemctl.rs`：持久化 systemd 状态的 fake systemctl。

fixture 独立于 `lkit-cli/src/install`，避免测试实现进入生产安装逻辑。

## Landscape v1 测试契约

共享契约定义在 `src/lib.rs` 的 `contract` 模块中。

### 启动参数

fake Landscape 接受 lkit 生成的真实参数：

```text
landscape-webserver \
  --config-dir <install-root>/data \
  --web <install-root>/current/static
```

`--config-dir` 必须提供；指定 `--web` 时，该目录必须存在。

测试场景配置优先由环境变量传入：

```text
LKIT_LANDSCAPE_FIXTURE_CONFIG=/path/to/landscape-fixture.json
```

未设置环境变量时读取 `<web>/lkit-fixture.json`。单个 Rust fixture 测试可使用环境
变量；Docker 与 nspawn release 使用 static 内配置。

### 初始化副作用

健康场景启动时只创建 `--config-dir` 下缺失的文件：

- `landscape_init.lock`；
- `landscape.toml`；
- `landscape_db.sqlite` 占位文件；
- `landscape_api_token`。

初始化锁、运行配置和数据库权限为 `0600`，API token 权限为 `0400`。已有文件不会在 restart 时被覆盖；若运行配置缺失但 `landscape_init.toml` 存在，则由 init 文件初始化运行配置。

`missing_init_artifacts` 场景不创建初始化锁和持久配置，用于验证 lkit 的初始化完成条件；数据库和 API token 仍可作为进程运行依赖创建。

### 网络行为

fake Landscape 在同一个进程内持有：

- DNS TCP listener；
- DNS UDP socket；
- HTTP listener；
- HTTPS listener。

端口和监听地址均由测试配置指定。普通用户测试通过 `test-support` 使用随机高位端口；
Docker 功能 E2E 与 nspawn 使用固定端口 `53`、`6300` 和 `6443`。

DNS TCP 接受连接但不处理 DNS 协议；DNS UDP 原样返回收到的数据。它们的主要作用是让 lkit 验证监听 socket 的 PID 所有权。

### HTTPS 与 API

fixture 启动时动态生成自签名 TLS 证书。

fixture 会显式安装 rustls AWS-LC crypto provider，避免它作为 `lkit-cli` 测试 wrapper 编译时因同时启用多个 rustls provider 而无法自动选择。

实现以下稳定端点：

```text
GET /api/docs
GET /api/v1/system/config/export
```

健康的 `/api/docs` 返回 `200`。

配置导出响应为：

```json
{
  "data": {
    "filename": "landscape_init_v0.22.0.toml",
    "version": "0.22.0",
    "content": "version = \"0.22.0\"\n"
  }
}
```

`filename` 和 `version` 来源于 fixture 配置；健康场景的 `content` 每次从当前 `landscape.toml` 读取。

## Landscape Fixture 配置

示例：

```json
{
  "schema_version": 1,
  "scenario": "healthy",
  "listen_address": "127.0.0.1",
  "dns_tcp_port": 21053,
  "dns_udp_port": 21053,
  "http_port": 21300,
  "https_port": 21443,
  "ready_delay_ms": 750,
  "exit_after_ms": 2000,
  "start_exit_code": 1,
  "export_version": "0.22.0",
  "export_content": "version = \"0.22.0\"\n"
}
```

约束：

- `schema_version` 当前必须为 `1`；
- 所有端口必须非零；
- `export_version` 必须是合法 semver；
- 未提供的可选字段使用 `LandscapeFixtureConfig::default()`。

## 故障场景

`scenario` 支持：

| 场景 | 行为 | 主要测试目标 |
| --- | --- | --- |
| `healthy` | 正常初始化并提供全部 listener 和 API | 首次安装成功 |
| `start_exit` | 启动后立即以配置的退出码退出 | systemd start 后进程失败 |
| `delayed_ready` | 等待 `ready_delay_ms` 后再初始化和监听 | 启动轮询与超时 |
| `missing_init_artifacts` | 不创建初始化锁和 `landscape.toml` | 初始化完成条件 |
| `health_error` | `/api/docs` 返回 `503` | HTTPS 健康检查失败 |
| `export_error` | 配置导出 API 返回 `500` | backup/upgrade 导出失败 |
| `exit_during_stability` | 就绪后经过 `exit_after_ms` 主动退出 | 稳定观察和失败清理 |

这些场景是声明式的，测试不需要在运行中调用控制 API，避免额外同步和竞态复杂度。

## Fake systemctl

fake systemctl 配置通过环境变量传入：

```text
LKIT_TEST_SYSTEMCTL_CONFIG=/path/to/systemctl-fixture.json
```

示例：

```json
{
  "schema_version": 1,
  "unit_dir": "/tmp/lkit-test/units",
  "state_dir": "/tmp/lkit-test/systemd-state",
  "landscape_config": "/tmp/lkit-test/landscape.json",
  "log_path": "/tmp/lkit-test/landscape.log",
  "call_log": "/tmp/lkit-test/systemctl-calls.jsonl",
  "systemd_version": "252.fixture"
}
```

当前实现支持 lkit 使用的命令：

```text
systemctl show --property=Version
systemctl show --property=ActiveState --value landscape-router.service
systemctl show --property=MainPID --value landscape-router.service
systemctl is-enabled landscape-router.service
systemctl is-active landscape-router.service
systemctl enable landscape-router.service
systemctl disable landscape-router.service
systemctl start landscape-router.service
systemctl stop landscape-router.service
systemctl restart landscape-router.service
systemctl daemon-reload
```

主要行为：

- 从 `unit_dir/landscape-router.service` 读取并解析 `ExecStart`；
- 启动真实的已安装 fake Landscape executable；
- `landscape_config` 非 null 时将该路径传递给 Landscape 子进程；为 null 时让进程从
  当前 release 的 static 目录读取配置；
- 每次调用以 JSON 数组追加到可选 `call_log`；
- 将 stdout/stderr 追加到 `log_path`；
- 原子记录 `MainPID`；
- 使用状态文件记录 enabled 状态；
- stop 时先发送 `SIGTERM`，超时后发送 `SIGKILL`；
- 查询时检测 PID 是否仍然存活。

## systemctl 退出码兼容性

真实 systemctl 的查询命令使用退出码表达状态：

- `is-active` 在 active 时返回 `0`，inactive 或 failed 时返回非零；
- `is-enabled` 在 enabled 时返回 `0`，disabled 时通常返回非零。

fake systemctl 按真实查询语义返回状态：inactive 使用退出码 `3`，disabled 使用退出码
`1`。lkit 同时区分“无法执行 systemctl”和“查询结果为 false”，避免替身掩盖生产问题。

## test-support feature 边界

fixture crate 已加入仓库根 Cargo workspace，并统一使用根 `Cargo.lock`。它不再包含独立 `[workspace]` 或独立锁文件。

`lkit-cli` 默认不依赖 fixture。只有显式启用 `test-support` feature 时，才会编译测试
运行时、`--test-runtime` 参数和以下 wrapper binaries：

```text
lkit-landscape-fixture
lkit-test-systemctl
```

## Release 内配置与版本

fixture 保持真实 Landscape 的启动参数，不增加测试专用 `--version`。配置来源按以下优先级解析：

1. `LKIT_LANDSCAPE_FIXTURE_CONFIG`；
2. `--web` 目录中的 `lkit-fixture.json`。

Docker release 将 `lkit-fixture.json` 放入 `static.zip` 的 `static/` 目录，使配置跟随 `current/static` 和 release 一起切换、一起回滚。

设置 `LKIT_FIXTURE_BUILD_VERSION` 编译时，fixture 仍可把版本嵌入二进制并在启动时校验。
Docker E2E 只编译一次 fixture，并由 release 生成器的 `--stamp-version` 在 ELF 尾部
写入版本标记，从而得到不同 SHA。

## 数据持久性

fixture 只创建缺失的数据文件，不在 restart 时覆盖现有数据库或配置。若 `landscape.toml` 缺失而 `landscape_init.toml` 存在，则使用 init 文件初始化运行配置。这使 `.lkb` 回滚后的 init 配置能够重新成为运行配置。

健康场景的配置导出 API 每次读取当前 `landscape.toml`，不返回启动时缓存的默认内容。fixture 同时创建权限为 `0400` 的 `landscape_api_token`，供真实 `lkit switch` 和 `repair` 读取。

## Release 生成器

`lkit-fixture-release` 负责生成 `lkit-publish` 所需的两份压缩后端资产和 `static.zip`：

```sh
lkit-fixture-release \
  --version 1.0.0 \
  --scenario healthy \
  --native-architecture x86_64 \
  --native-binary /path/to/landscape-webserver \
  --stamp-version \
  --output /path/to/dist
```

`--ready-delay-ms` 覆盖 `delayed_ready` 场景的启动延迟（默认 750 毫秒）；
Docker E2E 用 `10000` 触发测试运行时 4 秒启动轮询超时。

当前架构写入真实 binary，另一架构写入明确标记的不可运行占位资产。双架构 CI 分别在原生 x86_64 和 aarch64 runner 上验证真实资产。
