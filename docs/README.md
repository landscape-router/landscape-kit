# lkit-cli Landscape 管理规格

`lkit` 将首次安装、版本切换、修复、状态协调和服务管理器迁移暴露为独立子命令。命令层只负责参数与输出，具体行为由共享领域模块和 workflow 实现。

## 命令

- [`install`](commands/install.md)：首次安装。
- [`switch`](commands/switch.md)：切换到指定 stable 版本。
- [`repair`](commands/repair.md)：修复静态页面或后端二进制。
- [`reconcile`](commands/reconcile.md)：协调初始化完成状态、service unit 或仓库来源变化。
- [`service-manager`](commands/service-manager.md)：在 systemd 与外部进程管理之间迁移。
- [`network`](commands/network.md)：确认或回滚待定的网络接管。
- [`check`](check.md)：主机环境检查。

## 设计

- [lkit 自发布与安装入口](release/lkit.md)
- [安装布局与状态](deployment/layout-and-state.md)
- [事务与中断恢复](deployment/transactions-and-recovery.md)
- [初始化与凭据](interaction/initialization-and-credentials.md)
- [服务、进程与健康检查](service/runtime-and-health.md)
- [网络接管](network/takeover.md)
- [`.lkb` 备份与回滚](backup/lkb-and-rollback.md)
- [发布仓库协议](repository.md)
- [生命周期流程](workflows/lifecycle.md)
- [验收标准](acceptance.md)
- [测试体系](testing/README.md)
