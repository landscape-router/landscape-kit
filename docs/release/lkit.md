# `lkit` 自发布

## 版本来源与触发

根 `Cargo.toml` 的 `[workspace.package] version` 是 `lkit`（`lkit-cli`）及相关
workspace crate 的版本来源。成员 crate 通过 `version.workspace = true` 继承该版本；
`landscape-terrain-proto` 例外，它作为可独立发布的 Terrain L2 协议库使用自己的
版本线（当前 `0.1.0`），与 workspace 版本无关，也不参与 release tag 校验。
版本必须是 SemVer；候选版可以带 prerelease 后缀，发布 tag 固定为
`v<workspace.package.version>`。当前版本为 `0.6.2`。

发布前执行：

```sh
cargo fmt
cargo build --locked
cargo test --features test-support
scripts/test-install-lkit.sh
```

提交候选版本变更后创建并推送 prerelease tag：

```sh
git tag v0.6.2
git push origin v0.6.2
```

候选版本验证通过后，将 workspace 版本改为 `0.6.2`，再创建并推送正式 tag
`v0.6.2`。候选版和正式版必须使用两个独立的版本提交，tag、Cargo 版本与
`lkit --version` 必须完全一致。

`.github/workflows/release-lkit.yml` 会重新校验 tag、Cargo 版本和测试结果。任一架构构建
失败时不创建 Release；已存在的 Release 不允许由重跑覆盖。

`build/musl-static` 分支的 push 和手动触发会执行验证、四架构 Linux 静态构建及
Windows 客户端构建，并上传 Actions 产物；这两种入口不创建 GitHub Release。
只有推送 `v*` tag 才进入发布任务。分支构建使用 Cargo 包版本做二进制版本校验，
tag 构建还要求 tag 与包版本一致。Rust workspace 测试也在该分支 push 时运行。

## 发布产物

正式 Release 固定包含：

| 文件 | 内容 |
| --- | --- |
| `lkit-<arch>` | musl 全静态 Linux 二进制，arch 为 x86_64、aarch64、riscv64、loongarch64 |
| `lflare-linux-<arch>` | 同四种架构的 musl 全静态 Linux 客户端 |
| `lflare-windows-x86_64.exe` | Windows x86_64 裸二进制（Npcap 驱动） |
| `SHA256SUMS` | 上述二进制及安装脚本的 SHA-256 |
| `install.sh` | latest Release 安装入口 |

Linux 分发统一使用 musl 全静态编译，不构建或发布 glibc 版本，文件名不带 `-static` 后缀。
产物通过 cargo-zigbuild 交叉编译，目标集合与
Landscape 的 musl 发布对齐：`x86_64-unknown-linux-musl`、
`aarch64-unknown-linux-musl`、`riscv64gc-unknown-linux-musl`、
`loongarch64-unknown-linux-musl`。s390x 不在 musl 目标集合内。
Windows `lflare` 在原生 Windows runner 上构建（MSVC
目标），通过 `LIBPCAP_LIBDIR` 指向仓库内 vendor 的
[`landscape-flare/vendor/npcap-sdk`](../../landscape-flare/vendor/npcap-sdk) 链接
Npcap SDK，产物运行时要求目标机安装 Npcap。`rust-toolchain.toml` 固定编译器版本；
Cargo.lock 固定 Rust 依赖。

验证任务和每个架构的分发构建任务分别缓存 Cargo 下载及编译产物。分发缓存按架构隔离，
并随 Rust 工具链、依赖锁文件和相关构建输入失效；失败的任务也保存可复用缓存，避免重跑
Release 时从头编译。

分发二进制使用 workspace 的 `dist` profile：

```toml
[profile.dist]
inherits = "release"
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
```

该 profile 只用于分发的 `lkit` 与 `lflare`，不改变日常 `release` profile。CI 要求产物是已 strip 的
ELF、架构与构建目标一致、`lkit --version` 与 tag 一致。musl 产物必须没有
`PT_INTERP` 和 `DT_NEEDED`，跨架构产物通过 QEMU 运行版本及帮助命令，并在构建摘要中报告
实际体积；体积不设硬性上限。`panic=abort` 下意外 panic 等价于进程中断，后续命令仍
通过既有事务记录恢复。

## 本地 musl 静态打包

依赖 Rust（仓库 `rust-toolchain.toml`）、Python 3、Zig、cargo-zigbuild 和 GNU binutils：

```sh
python3 -m pip install --user ziglang==0.16.0
cargo install --locked cargo-zigbuild --version 0.23.4
# 默认按宿主架构选择 musl 目标
bash scripts/build-musl-static.sh
# 指定目标交叉编译
bash scripts/build-musl-static.sh x86_64-unknown-linux-musl
bash scripts/build-musl-static.sh aarch64-unknown-linux-musl
bash scripts/build-musl-static.sh riscv64gc-unknown-linux-musl
bash scripts/build-musl-static.sh loongarch64-unknown-linux-musl
```

脚本安装目标 Rust 标准库，使用 `dist` profile 构建 `lkit` 和 `lflare`，验证 ELF 架构及
静态链接后将 `lkit-<arch>`、`lflare-linux-<arch>` 及 `SHA256SUMS` 放入 `dist/`
（可用 `OUTPUT_DIR` 修改）。日常开发与单元测试的 `cargo build/test` 使用宿主工具链，
不作为分发产物。无需 Landscape 的
libelf/sysroot：本项目的 Linux 客户端使用 AF_PACKET，不链接 libpcap。

部署状态与备份记录支持这四种架构。GitHub Landscape 仓库在 riscv64、loongarch64 上
使用 `landscape-webserver-<arch>-static`；x86_64、aarch64 保留其既有资产选择。
HTTP 仓库 manifest 的 `assets.webserver` 可声明可选的 `riscv64`、`loongarch64` 项，
缺少对应资产时明确报错，不回退到其他架构。内置镜像发布器目前仍发布 x86_64/aarch64。

## 安装入口

只安装最新版 `lkit` 到 `/usr/local/bin/lkit`：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

已安装环境的升级使用 `lkit self upgrade`：它从 GitHub Release 下载对应架构二进制与
`SHA256SUMS`，按与 install.sh 相同的规则校验、自检并原子替换 `/usr/local/bin/lkit`，
并在 daemon 注册且运行时 restart 使其加载新二进制（见 [`lkit self`](../commands/self.md)）。

发布时 `install.sh` 内的下载地址会被替换为对应 Release 的资产地址
（`releases/download/<tag>/`），不依赖 `releases/latest` 的指向。因此每个 Release
的 `install.sh` 始终安装该 Release 自身的内容；`releases/latest/download/install.sh`
是 GitHub 固定的入口，指向最新 stable Release。候选版（`-rc.*`）是 prerelease，
`releases/latest` 不会指向它，安装候选版必须使用带 tag 的地址：

```sh
wget -qO- https://github.com/landscape-router/landscape-kit/releases/download/v0.6.2/install.sh | sudo sh
```

交互式安装推荐分两步执行，确保 `lkit` 直接连接当前终端的 `/dev/tty`：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
sudo lkit
```

裸命令进入 Ratatui 管理控制台；自动化仍使用显式 `lkit install` 子命令。

显式使用 Landscape 镜像仓库安装：

```sh
sudo lkit install --repository https://l1s3.whileaway.dev/landscape/
```

`install` 后面的参数原样传给 `lkit install`。安装器和 `lkit self upgrade` 选择 Linux `x86_64`、`aarch64`、`riscv64`、
`loongarch64` 的 `lkit-<arch>` 静态产物，不依赖宿主 glibc 或 musl 动态加载器。
这不表示所有发行版的系统服务、网络后端或可选软件都受支持；这些功能仍受各自平台限制。
安装器自动选择下载工具：
优先使用 `curl`，缺失时回退 `wget`，两者都不可用时明确报错。下载工具不存在时，
`wget -qO- … | sudo sh` 拉取安装脚本的用法仍然无效。安装器强制使用 HTTPS，
下载对应二进制和 `SHA256SUMS`，校验成功并执行
`lkit --version` 自检后才原子替换目标文件。下载、校验、自检或替换失败时保留原有
二进制。

为兼容已有调用，安装脚本仍接受 `install` 及其后续参数并原样转发；只有调用环境确实
提供 `/dev/tty` 时才能交互输入密码。管道和标准输入不会成为密码输入源。无 TTY 的
自动化环境必须使用权限受限的 `--password-file`，安装脚本不增加明文密码参数。

SHA-256 防止下载损坏或资产不一致；其信任来源仍是 GitHub Release 和 HTTPS。首版不使用
UPX、nightly `build-std`、GPG 签名或 artifact attestation。
