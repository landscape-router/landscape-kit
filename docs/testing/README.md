# 测试体系

`landscape-kit` 按被验证的能力分层。完整业务生命周期不要求真实 systemd；真实
systemd 只承担协议与进程托管的薄集成验证。

| 层次 | 入口 | 环境 | 默认频率 | 主要覆盖 |
| --- | --- | --- | --- | --- |
| Rust 单元与 fixture E2E | `cargo test --features test-support` | 普通用户 | 提交前（当前由开发者执行） | workflow、fake systemctl、健康检查、失败清理 |
| lkit 安装器 | `scripts/test-install-lkit.sh` | 普通用户、命令替身 | 提交前、正式 tag | 架构选择、校验、原子替换、参数透传 |
| RustFS 发布集成 | `scripts/test-publish-http-repository.sh` | Docker | `dev`、`main`、手动 | S3 发布、manifest、stable pointer、失败原子性 |
| Docker 功能 E2E | `scripts/test-docker-lifecycle.sh` | 普通 Docker 容器、fake systemctl | `dev`、`main`、手动 | S1-S4、S6-S9 安装、切换、备份、回滚和迁移 |
| systemd-nspawn 兼容性 smoke | `scripts/test-nspawn-systemd.sh` | root、真实 systemd PID 1 | 低频、手动或 systemd 契约变化时 | unit 注册启停、MainPID、systemd worker、前端断连 |

## 核心功能测试

Docker 功能 E2E 使用 `test-support` 构建，并显式配置：

```json
{
  "preflight": "skip",
  "execution": "inline"
}
```

首次安装仍选择 `--service-manager systemd`，后续命令保持已提交的 systemd 模式，并
通过现有 fake systemctl 启动真实安装后的 fixture 进程；跳过的是宿主内核、BPF、依赖
和 PID 1 能力审计。Docker 因而无需
`privileged`、cgroup 委托、`/boot` 挂载或 systemd PID 1。

首次安装、版本切换、修复和回滚是否正确，以 Rust fixture E2E 与 Docker 功能 E2E
执行的真实 CLI、文件系统变更、进程启停和健康检查为主要证据。fake systemctl 在这一层
是隔离外部 service manager 的测试替身，不会把下载、校验、事务、初始化或进程验证
降级为模拟结果。

生产构建不包含 `--test-runtime`，PID 1、`/run/systemd/system`、systemctl 可执行性和
manager 可达性要求没有放宽。

## systemd 兼容性 Smoke

nspawn 层配置 `preflight: skip` 和 `execution: systemd_worker`，只验证真实 systemd
行为。完整宿主预检由 Debian/QEMU 或实际受支持主机的验收负责，不与业务状态机矩阵
重复绑定。

nspawn 不是上述功能矩阵的严格 systemd 重跑，也不是常规 PR 或发布门禁。它只抽样验证
无法由 fake systemctl 证明的 systemd 兼容性契约，例如 unit 能被真实 manager 接受、
MainPID 语义和临时 worker 在前端断开后的生命周期。该 smoke test 适合低频手动运行，
或在 unit、systemd worker、systemctl 协议适配发生变化时运行；不要求每个业务场景都在
真实 systemd 下重复执行，避免把宿主环境波动和 rootfs 启动成本变成发布阻塞条件。

## 文档

- [产品测试场景总目录](scenarios/README.md)
- [Fake Landscape fixture](fixture.md)
- [Docker 功能 E2E](docker-e2e.md)
- [systemd-nspawn 兼容性 smoke](nspawn-systemd.md)
- [发布、安装与成功切换](scenarios/lifecycle.md)
- [失败切换与自动回滚](scenarios/rollback.md)
- [扩展 E2E 场景](scenarios/extended.md)

当前 `.lkb` 是 minimal 配置级备份，不包含 `landscape_db.sqlite`。数据库级备份恢复和
公开的 `lkit backup`、`lkit restore` 命令属于后续阶段。
