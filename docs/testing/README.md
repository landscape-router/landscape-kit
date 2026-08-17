# 测试体系

`landscape-kit` 按被验证的能力分层。完整业务生命周期不要求真实 systemd；真实
systemd 只承担协议与进程托管的薄集成验证。

| 层次 | 入口 | 环境 | 默认频率 | 主要覆盖 |
| --- | --- | --- | --- | --- |
| Rust 单元与 fixture E2E | `cargo test --workspace --features test-support` | 普通用户 | 相关 PR、`dev`/`main`、手动 | workflow、fake systemctl、健康检查、失败清理 |
| lkit 安装器 | `scripts/test-install-lkit.sh` | 普通用户、命令替身 | 提交前、正式 tag | 架构选择、校验、原子替换、参数透传 |
| RustFS 发布集成 | `scripts/test-publish-http-repository.sh` | Docker | `dev`、`main`、手动 | S3 发布、manifest、stable pointer、失败原子性 |
| Docker 功能 E2E | `scripts/test-docker-lifecycle.sh` | 普通 Docker 容器、fake systemctl | `dev`、`main`、手动 | S1-S4、S6-S10 安装、切换、备份、恢复、回滚、迁移、reconcile 和卸载 |
| Docker 换源 E2E | `scripts/test-docker-mirrors.sh` | Debian/Ubuntu/Fedora/Arch 官方镜像容器 | 相关 PR、`dev`、`main`、手动 | `set-mirror` 切换、备份、恢复与 CD 源兜底 |
| Docker 常用软件 E2E | `scripts/test-docker-software.sh` | Debian/Ubuntu/Fedora/Arch 官方镜像容器 | 相关 PR、`dev`、`main`、手动 | `software install docker` 仓库配置、真实软件包安装与服务启用契约 |
| systemd-nspawn 兼容性 smoke | `scripts/test-nspawn-systemd.sh` | root、真实 systemd PID 1 | 低频、手动或 systemd 契约变化时 | unit 注册启停、MainPID、systemd worker、前端断连 |
| QEMU 网络接管 | `scripts/test-qemu-network-takeover.sh` | GitHub-hosted x86_64 KVM、双 virtio 网卡 | 相关 PR、main、每周、手动 | 真实宿主网络服务、br_lan SSH 确认、未确认重启回滚 |
| 真实 ifupdown 兼容 | `cargo test -p lkit-hostnet --test ifupdown_real` | Debian ifupdown 容器 | 相关 PR、`dev`/`main`、手动 | ifup/ifquery 脚本生成、备份恢复、命令失败回滚 |

## 脚本与 CI 结构

每个测试域（domain）三处命名一致：

| 位置 | 命名 |
| --- | --- |
| workflow | `.github/workflows/test-<domain>.yml` |
| 入口脚本 | `scripts/test-<domain>.sh` |
| Docker 构建器 | `scripts/docker/<domain>/` |

域一览：

| 域 | workflow | 入口脚本 | 构建器目录 | 说明 |
| --- | --- | --- | --- | --- |
| rust | `test-rust.yml` | （无，直接 cargo） | （无） | fmt、clippy、单元测试、i18n |
| fixture-e2e | `test-fixture-e2e.yml` | （无，直接 cargo） | （无） | Rust fixture E2E 套件，需 `LKIT_E2E=1` |
| docker-lifecycle | `test-docker-lifecycle.yml` | `test-docker-lifecycle.sh` | `docker/lifecycle/` | compose 双容器（rustfs + e2e）功能 E2E |
| docker-software | `test-docker-software.yml` | `test-docker-software.sh` | `docker/software/` | 多发行版常用软件安装 E2E |
| docker-mirrors | `test-docker-mirrors.yml` | `test-docker-mirrors.sh` | `docker/mirrors/` | 多发行版换源 E2E |
| hostnet-ifupdown | `test-hostnet-ifupdown.yml` | （无，直接 cargo） | （无） | 真实 ifupdown 兼容性 |
| qemu-network-takeover | `test-qemu-network-takeover.yml` | `test-qemu-network-takeover.sh` | （无） | KVM 双网卡网络接管 |
| nspawn-systemd | `test-nspawn-systemd.yml` | `test-nspawn-systemd.sh` | （无） | 真实 systemd PID 1 smoke |
| publish-http-repository | `test-publish-http-repository.yml` | `test-publish-http-repository.sh` | （无） | RustFS 发布集成 |

`scripts/lib/` 是共享库（如 `rustfs-test.sh`，供 `docker/lifecycle` 的 `run-service.sh` 使用）；
`scripts/install-lkit.sh` 与 `scripts/test-install-lkit.sh` 属于 install 域，由 `release-lkit.yml`
在正式 tag 时校验并随产物发布。

触发约定：所有 `test-*` workflow 在 PR 与 push（`dev`/`main`）上按 paths 过滤触发，并支持
`workflow_dispatch` 手动运行；`qemu-network-takeover` 另有每周 cron，`nspawn-systemd` 有每周
cron；`release-lkit.yml` 由 `v*` tag 触发，`publish-landscape-mirror.yml` 仅手动触发。

fixture E2E 套件（`tests/install_fixture_e2e`）会在宿主机上部署真实服务并生成真实进程，
只有显式设置 `LKIT_E2E=1` 时才执行；本地误跑（如被 `daemon::` 这类子串过滤器匹配）
会在宿主机挂起并泄漏进程，CI 的两个入口（`test-fixture-e2e.yml`、`release-lkit.yml`）都已设置
该变量。

## 核心功能测试

Docker 功能 E2E 使用 `test-support` 构建，并显式配置：

```json
{
  "preflight": "skip",
  "execution": "inline"
}
```

首次安装使用 systemd 模式，后续命令保持已提交的 systemd 模式，并通过现有 fake
systemctl 启动真实安装后的 fixture 进程；跳过的是宿主内核、BPF、依赖
和 PID 1 能力审计。Docker 因而无需
`privileged`、cgroup 委托、`/boot` 挂载或 systemd PID 1。

首次安装、版本切换、备份/恢复、修复和回滚是否正确，以 Rust fixture E2E 与 Docker 功能 E2E
执行的真实 CLI、文件系统变更、进程启停和健康检查为主要证据。fake systemctl 在这一层
是隔离外部 service manager 的测试替身，不会把下载、校验、事务、初始化或进程验证
降级为模拟结果。

生产构建不包含 `--test-runtime`，PID 1、`/run/systemd/system`、systemctl 可执行性和
manager 可达性要求没有放宽。

## systemd 兼容性 Smoke

nspawn 层配置 `preflight: skip` 和 `execution: daemon`，只验证真实 systemd
行为。完整宿主预检由 glibc Linux VM 或实际受支持主机的验收负责，不与业务状态机矩阵
重复绑定。

nspawn 不是上述功能矩阵的严格 systemd 重跑，也不是常规 PR 或发布门禁。它只抽样验证
无法由 fake systemctl 证明的 systemd 兼容性契约，例如 unit 能被真实 manager 接受、
MainPID 语义和临时 worker 在前端断开后的生命周期。该 smoke test 适合低频手动运行，
或在 unit、systemd worker、systemctl 协议适配发生变化时运行；不要求每个业务场景都在
真实 systemd 下重复执行，避免把宿主环境波动和 rootfs 启动成本变成发布阻塞条件。

QEMU 层覆盖 nspawn 无法验证的真实网卡接管。它要求 `/dev/kvm`，不使用 TCG fallback；
初期 check 保持 observational。最近 20 次已完成运行全部成功后，workflow 会报告达到
提升条件，再由仓库管理员将其加入 branch protection required checks。

## 文档

- [产品测试场景总目录](scenarios/README.md)
- [Fake Landscape fixture](fixture.md)
- [Docker 功能 E2E](docker-lifecycle.md)
- [Docker 常用软件安装 E2E](docker-software.md)
- [systemd-nspawn 兼容性 smoke](nspawn-systemd.md)
- [QEMU/KVM 网络接管](qemu-network-takeover.md)
- [发布、安装与成功切换](scenarios/lifecycle.md)
- [失败切换与自动回滚](scenarios/rollback.md)
- [扩展 E2E 场景](scenarios/extended.md)

当前 `.lkb` 是 minimal 配置级备份，不包含 `landscape_db.sqlite`。公开的 `lkit backup`
和 `lkit restore` 只覆盖已有安装内的配置级恢复；数据库级备份恢复和空目录灾难重建仍
属于后续阶段。卸载（`lkit uninstall`）已有独立的场景文档
[uninstall.md](scenarios/functional/uninstall.md)，其卸载前保护 `.lkb` 同样不包含
SQLite 数据文件。
