# lkit-cli Landscape 管理规格

`lkit` 裸命令提供 Ratatui 管理控制台，同时将首次安装、版本更新与切换、修复、状态协调和
服务管理器迁移暴露为独立子命令。控制台和命令层只负责输入与输出，具体行为由共享领域模块
和 workflow 实现。

## 交互控制台

- [Ratatui 管理控制台](interaction/console.md)：侧栏、安装表单、命令模式边界和终端恢复。
- [命令行本地化](interaction/i18n.md)：`en`/`zh` 选择、优先级和稳定机器契约。

## 命令

- [`install`](commands/install.md)：首次安装。
- [`update`](commands/update.md)：交互式更新到最新或指定 stable 版本。
- [`switch`](commands/switch.md)：切换到指定 stable 版本。
- [`backup`](commands/backup.md)：创建、查看和验证 `.lkb` minimal 备份。
- [`restore`](commands/restore.md)：在现有安装内从 `.lkb` 恢复版本和配置。
- [`uninstall`](commands/uninstall.md)：卸载已安装的 Landscape 并清理受管文件。
- [`repair`](commands/repair.md)：修复静态页面或后端二进制。
- [`reconcile`](commands/reconcile.md)：协调初始化完成状态、service unit 或仓库来源变化。
- [`service-manager`](commands/service-manager.md)：在 systemd 与外部进程管理之间迁移。
- [`network`](commands/network.md)：确认或回滚待定的网络接管。
- [`check`](check.md)：主机环境检查。

## 设计

- [lkit 自发布与安装入口](release/lkit.md)
- [安装布局与状态](deployment/layout-and-state.md)
- [配置文件（`config.toml`）](deployment/config.md)
- [事务与中断恢复](deployment/transactions-and-recovery.md)
- [初始化与凭据](interaction/initialization-and-credentials.md)
- [服务、进程与健康检查](service/runtime-and-health.md)
- [网络接管](network/takeover.md)
- [`.lkb` 备份与回滚](backup/lkb-and-rollback.md)
- [发布仓库协议](repository.md)
- [生命周期流程](workflows/lifecycle.md)
- [验收标准](acceptance.md)
- [测试体系](testing/README.md)
