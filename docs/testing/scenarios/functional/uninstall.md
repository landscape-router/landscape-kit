# 卸载场景

## UNI-01

**systemd 模式卸载：停止、禁用、注销服务并清理受管文件**

- 测试层：Rust workflow、Docker E2E
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)、[生命周期 9. 卸载](../../../workflows/lifecycle.md#9-卸载)
- 说明：验证 stop → disable → 注销注册链接 → daemon-reload → 删除受管内容 → 事务
  `committed`；保护 `.lkb` 落盘在 `backups/`，卸载成功后输出备份 ID 与保留物清单。

## UNI-02

**systemd 模式卸载：停止、disable 并注销受管服务后删除受管内容**

- 测试层：Rust workflow、CLI E2E
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#执行与提交)、`uninstalls_none_mode_and_keeps_config_backups_transactions`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）

## UNI-03

**非交互模式缺少 `--yes` 时返回参数错误 `2`，不创建事务、不写文件**

- 测试层：CLI
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)、[输出与退出码](../../../commands/output-and-exit-codes.md)
- 说明：`--yes` 覆盖卸载计划、数据损失与网络接管警告确认。

## UNI-04

**保护 `.lkb` 创建失败时阻断卸载，服务与现场保持不变**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#卸载前检查)
- 说明：事务未创建或标记 `failed`，已提交安装不被破坏。

## UNI-05

**`--allow-no-backup` 跳过保护备份并记录 `no_backup: true`**

- 测试层：Rust workflow、Docker E2E
- 状态：`待补充`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md)

## UNI-06

**默认卸载保留 `config.toml`、`backups/` 与 `transactions/`，删除其余受管内容**

- 测试层：Rust workflow、Docker E2E
- 状态：`待补充`
- 证据：[卸载保留物](../../../deployment/layout-and-state.md#卸载保留物)
- 说明：`config.toml` 内容逐字节不变；保护 `.lkb` 与卸载事务文件仍可读。

## UNI-07

**`--keep-data` 保留 `data/`，其余受管内容删除且安装视为已卸载**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#清理选项)
- 说明：`state/install-state.json` 被删除；再次 `lkit install` 视为全新首次安装。

## UNI-08

**`--purge-root` 整树删除安装根目录（含 `config.toml`），必须同时给出 `--allow-no-backup`**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#清理选项)

## UNI-09

**`--purge-root` 缺 `--allow-no-backup` 或与 `--keep-data` 组合时返回参数错误 `2`**

- 测试层：CLI
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#清理选项)

## UNI-10

**网络接管特征（宿主网络服务被 stop/disable/mask）警告后仍可继续卸载**

- 测试层：Rust workflow、Docker E2E
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#卸载前检查)
- 说明：检测接管特征并输出警告，不阻断；卸载后宿主网络服务保持现状，由用户自行恢复。

## UNI-11

**卸载中断后下次命令前向完成，不自动回滚**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[中断恢复](../../../deployment/transactions-and-recovery.md)
- 说明：在 `stopping` 或 `activating` 阶段中断，下次任意 lkit 命令继续完成注销、删除
  与提交；恢复再次失败标记 `failed` 并保留保护 `.lkb` 与事务现场。

## UNI-12

**卸载成功后同一根目录可再次执行全新首次安装**

- 测试层：Rust workflow、Docker E2E
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#执行与提交)
- 说明：`install-state.json` 不存在后 `lkit install` 按 FirstInstall 处理并成功。

## UNI-13

**未安装或状态损坏时拒绝卸载**

- 测试层：Rust workflow、CLI
- 状态：`待补充`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)
- 说明：无有效 state 返回参数错误 `2`；状态损坏按损坏判定阻断，不猜测重建。

## UNI-14

**控制台 Uninstall 面板确认、委托与结果展示（暂隐藏，代码保留）**

- 测试层：控制台集成测试
- 状态：`部分覆盖`
- 证据：[Ratatui 管理控制台](../../../interaction/console.md)
- 说明：面板当前从侧栏隐藏（`Menu::ALL` 中 `Self::Uninstall` 注释保留），只经 CLI
  `lkit uninstall` 使用；面板渲染、确认层与委托的 Rust 单测保留并通过，重新启用后
  需补充 TUI 端到端场景。确认层展示版本、数据损失范围、保留物与网络接管警告；
  `--console-confirmed` 委托共享命令分发，systemd 模式经 worker 执行，成功展示备份
  ID 与保留物清单。
