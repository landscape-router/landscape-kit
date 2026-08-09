# Landscape Kit

`lkit` 是用于管理 [Landscape](https://landscape.canonical.com/) 实例的交互终端控制台和命令行工具：支持首次安装、版本切换、修复、状态协调与服务管理器迁移。

本仓库是一个 Cargo workspace，包含四个 crate：

| Crate | 职责 |
| --- | --- |
| `crates/lkit-cli` | `lkit` 二进制：命令层、领域逻辑与 workflow |
| `crates/lkit-publish` | `lkit-publish` 二进制：打包发布并发布到仓库 |
| `crates/lkit-repository` | 仓库协议库，CLI 与发布器共享 |
| `crates/lkit-test-fixture` | 测试 fixture：模拟 `systemctl`、HTTPS webserver 与测试仓库 |

## 命令

- `check` — 主机环境检查。
- `install` — 首次安装。
- `switch` — 切换到指定 stable 版本。
- `backup` — 创建、查看和验证 `.lkb` minimal 备份。
- `restore` — 在现有安装内从 `.lkb` 恢复版本和配置。
- `repair` — 修复静态页面或后端二进制。
- `reconcile` — 接受并记录初始化文件、service unit 或仓库来源变化。
- `service-manager` — 在 systemd 与外部进程管理之间迁移。

## 文档

规格与设计文档见 [`docs/`](docs/README.md)。本说明的英文版见 [README.md](README.md)。

## 安装 Landscape

当前支持使用 glibc 的 Linux `x86_64` 和 `aarch64` 主机。先安装最新版 `lkit`，再从终端
直接进入 Landscape 交互式安装：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

也可以使用 `wget`：

```sh
wget -qO- https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

安装器自身会自动选择下载工具：优先使用 `curl`，缺失时回退 `wget`，主机上只需二者之一
即可完成安装。

然后启动交互式安装：

```sh
sudo lkit
```

裸命令进入 Ratatui 管理控制台。脚本和 CI 应使用明确子命令，例如
`lkit --non-interactive install ...`。

界面会跟随系统 locale，支持英文和简体中文。使用 `lkit --lang zh ...` 或设置
`LKIT_LANG=zh` 可覆盖系统设置；不支持的语言回退到英文。

使用 Landscape 镜像仓库安装：

```sh
sudo lkit install --repository https://l1s3.whileaway.dev/landscape/
```

安装器会根据架构选择二进制，通过 Release 的 `SHA256SUMS` 校验后原子安装到
`/usr/local/bin/lkit`。发行版名称不再使用白名单；部署前由 `lkit` 检查内核和实际运行
能力。当前发布二进制不支持 Alpine 等 musl 发行版。发布产物、版本规则和手动发布步骤见
[`lkit` 自发布规范](docs/release/lkit.md)。

## 构建与测试

```sh
cargo build --locked
cargo test --features test-support
```

依赖 fixture 二进制的测试需要启用 `test-support` feature。RustFS 发布集成测试不混入 `cargo test`，单独运行：

```sh
RUSTFS_IMAGE=<固定镜像> scripts/test-publish-http-repository.sh
```
Docker 功能 E2E 可在 Linux x86_64 本地运行；原生 aarch64 覆盖由 CI 执行：

```sh
scripts/test-docker-lifecycle.sh
```

测试分层及低频/手动执行的真实 systemd nspawn 兼容性 smoke test 见
[`docs/testing/README.md`](docs/testing/README.md)。
