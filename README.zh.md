# Landscape Kit

Landscape Kit 提供用于安装和运维 [Landscape](https://github.com/ThisSeanZhang/landscape) 实例的 `lkit` 终端控制台和命令行工具，覆盖首次部署、手工部署迁移、更新、版本切换、修复、备份、网络接管和服务生命周期管理。

本 workspace 还包含独立的 `lflare` 客户端，用于 Landscape Terrain L2 防失联通道。在路由器的常规 IP 路径不可用时，可以通过它建立应急管理连接。

## 快速开始

当前发布二进制支持使用 glibc 的 Linux `x86_64` 和 `aarch64`。不支持 Alpine 等基于 musl 的发行版。安装器需要主机提供 `curl` 或 `wget`，并具备 `sudo` 权限：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

也可以使用：

```sh
wget -qO- https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

在终端启动交互式管理控制台：

```sh
sudo lkit
```

裸 `lkit` 命令进入 Ratatui 控制台。自动化场景应使用明确的子命令和 `--non-interactive`，例如：

```sh
sudo lkit --non-interactive check
sudo lkit --non-interactive install --password-file /root/lkit-password
```

指定 Landscape 仓库安装：

```sh
sudo lkit install --repository https://l1s3.whileaway.dev/landscape/
```

界面跟随系统 locale，支持英文和简体中文。可使用 `--lang en`、`--lang zh` 或 `LKIT_LANG` 覆盖语言选择。

安装器会根据 Release 的 `SHA256SUMS` 校验下载内容，并将 `lkit` 原子安装到 `/usr/local/bin/lkit`。支持的平台、升级行为和安全细节见 [`lkit` 发布与安装规范](docs/release/lkit.md)。

## 常用命令

完整参数、确认规则和失败恢复方式请查看对应的命令文档。

| 领域 | 命令 |
| --- | --- |
| 检查与安装 | [`check`](docs/check.md)、[`install`](docs/commands/install.md)、[`migrate`](docs/commands/migrate.md) |
| 版本与修复 | [`update`](docs/commands/update.md)、[`switch`](docs/commands/switch.md)、[`repair`](docs/commands/repair.md)、[`reinit`](docs/commands/reinit.md) |
| 备份与状态 | [`backup`](docs/commands/backup.md)、[`restore`](docs/commands/restore.md)、[`reconcile`](docs/commands/reconcile.md) |
| 网络与主机设置 | [`network`](docs/commands/network.md)、[`set-mirror`](docs/commands/mirror.md)、[`software`](docs/commands/software.md) |
| 卸载与 lkit 服务 | [`uninstall`](docs/commands/uninstall.md)、[`self`](docs/commands/self.md) |

`lkit self install` 会将 lkit daemon 注册为 systemd 服务。`lkit self upgrade` 更新 lkit 二进制并重新加载已注册的 daemon；`lkit self remove` 只注销该 daemon，不删除 lkit CLI 或 Landscape 数据。

## Terrain 防失联通道

Terrain 是主机与 Landscape 路由器之间的加密二层应急通道。`lkit` daemon 可以托管服务端，使用 `lkit flare setup` 配置或查看恢复密钥。

`lflare` 默认进入交互式客户端。脚本可以使用 `cli` 子命令：

```sh
lflare cli --psk '<恢复密钥>' --dev eth0 --forward 2222:22
```

Linux 客户端需要受支持的 glibc 目标；Windows 客户端需要安装 Npcap。协议细节、配置方式和端到端场景见 [Terrain 文档](docs/flare/README.md)。

## Workspace

| Crate | 职责 |
| --- | --- |
| `lkit-cli` | `lkit` 二进制：控制台、命令、workflow 与 daemon |
| `landscape-flare` | `lflare` Terrain 防失联客户端 |
| `landscape-terrain-proto` | Terrain L2 协议与传输库 |
| `crates/lkit-hostnet` | 主机网络适配与回滚库 |
| `crates/lkit-publish` | `lkit-publish`，发布仓库的发布器 |
| `crates/lkit-repository` | CLI 与发布器共享的仓库协议类型 |
| `crates/lkit-test-fixture` | 测试使用的隔离 fixture 二进制，不是运行时依赖 |

## 构建与测试

构建完整 workspace：

```sh
cargo build --locked --workspace
```

本地聚焦检查时，对改动模块运行格式化、Clippy 和单元测试：

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test -p lkit-cli --features test-support --bin lkit <module-filter>
```

测试体系按层划分。Docker、systemd、QEMU、发布和 Terrain 场景都有专用环境及 CI workflow；在本地运行前请先阅读[测试指南](docs/testing/README.md)。

## 文档与贡献

- [文档索引](docs/README.md)
- [`lkit` 发布与安装](docs/release/lkit.md)
- [测试指南](docs/testing/README.md)
- [贡献指南](CONTRIBUTING.zh.md) · [English contributing guide](CONTRIBUTING.md)

Issue 不是必需的。提交代码变更时，请遵循贡献指南中的工作流和测试要求。
