# `lkit` 自发布

## 版本来源与触发

`crates/lkit-cli/Cargo.toml` 中的 `package.version` 是 `lkit` 自身版本的唯一来源。
正式版本只接受不带 prerelease 的 SemVer，发布 tag 固定为 `v<package.version>`。

发布前执行：

```sh
cargo fmt
cargo build --locked
cargo test --features test-support
scripts/test-install-lkit.sh
```

提交版本变更后创建并推送 tag，例如：

```sh
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release-lkit.yml` 会重新校验 tag、Cargo 版本和测试结果。任一架构构建
失败时不创建 Release；已存在的 Release 不允许由重跑覆盖。

## 发布产物

正式 Release 固定包含：

| 文件 | 内容 |
| --- | --- |
| `lkit-x86_64` | Debian GNU/Linux x86_64 裸二进制 |
| `lkit-aarch64` | Debian GNU/Linux aarch64 裸二进制 |
| `SHA256SUMS` | 两个二进制及安装脚本的 SHA-256 |
| `install.sh` | latest Release 安装入口 |

两种架构都在原生 GitHub runner 上、固定的 Rust Bookworm 容器中构建，不使用交叉编译。
`rust-toolchain.toml` 固定编译器版本；Cargo.lock 固定 Rust 依赖。

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

该 profile 只用于分发的 `lkit`，不改变日常 `release` profile。CI 要求产物是已 strip 的
动态链接 ELF、架构与 runner 一致、`lkit --version` 与 tag 一致，并且每个文件不超过
`5 MiB`。`panic=abort` 下意外 panic 等价于进程中断，后续命令仍通过既有事务记录恢复。

## 安装入口

只安装最新版 `lkit` 到 `/usr/local/bin/lkit`：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh
```

安装 `lkit` 后立即进入现有的 Landscape 交互式安装：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh -s -- install
```

显式使用 Landscape 镜像仓库安装：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/landscape-router/landscape-kit/releases/latest/download/install.sh | sudo sh -s -- install --repository https://l1s3.whileaway.dev/landscape/
```

`install` 后面的参数原样传给 `lkit install`。安装器只支持 Linux `x86_64` 和
`aarch64`，强制使用 HTTPS，下载对应二进制和 `SHA256SUMS`，校验成功并执行
`lkit --version` 自检后才原子替换目标文件。下载、校验、自检或替换失败时保留原有
二进制。

管道不会成为密码输入源。`lkit install` 仍从 `/dev/tty` 隐藏读取并确认管理员密码；
无 TTY 的自动化环境仍必须使用权限受限的 `--password-file`，安装脚本不增加明文密码
参数。

SHA-256 防止下载损坏或资产不一致；其信任来源仍是 GitHub Release 和 HTTPS。首版不使用
UPX、nightly `build-std`、GPG 签名或 artifact attestation。
