# 卸载场景

`lkit uninstall` 只卸载 landscape 安装根;lkit 地盘(`/root/.lkit/`)与 lkit 常驻
daemon 不属于卸载范围(见 [`lkit self`](self.md))。

## UNI-01

**systemd 模式卸载：停止、禁用、注销服务并清理受管文件**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)、[生命周期 9. 卸载](../../../workflows/lifecycle.md#9-卸载)、
  `systemd_mode_unregisters_the_unit_but_not_lkit_daemon`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）、
  `uninstalls_an_existing_installation_through_full_cli`（crates/lkit-cli/tests/install_fixture_e2e/uninstall.rs）
- 说明：验证 stop → disable → 注销注册链接 → daemon-reload → 删除 landscape 根受管
  内容 → 事务 `committed`；保护 `.lkb` 落盘在 lkit 地盘 `backups/`，卸载成功后输出
  备份 ID 与保留物清单。

## UNI-02

**systemd 模式卸载：停止、disable 并注销受管服务后删除受管内容**

- 测试层：Rust workflow、CLI E2E
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#执行与提交)、
  `systemd_mode_unregisters_the_unit_but_not_lkit_daemon`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）、
  `uninstalls_an_existing_installation_through_full_cli`（crates/lkit-cli/tests/install_fixture_e2e/uninstall.rs）

## UNI-03

**非交互模式缺少 `--yes` 时返回参数错误 `2`，不创建事务、不写文件**

- 测试层：CLI
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)、[输出与退出码](../../../commands/output-and-exit-codes.md)、
  `requires_yes_in_non_interactive_mode`（crates/lkit-cli/src/workflows/uninstall/mod.rs）
- 说明：`--yes` 覆盖卸载计划、数据损失与网络接管警告确认。

## UNI-04

**保护 `.lkb` 创建失败时阻断卸载，服务与现场保持不变**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#卸载前检查)、
  `blocks_without_allow_no_backup_when_protection_fails`（crates/lkit-cli/src/workflows/uninstall/mod.rs）
- 说明：事务未创建或标记 `failed`，已提交安装不被破坏。

## UNI-05

**`--allow-no-backup` 跳过保护备份并记录 `no_backup: true`**

- 测试层：Rust workflow、Docker E2E
- 状态：`部分覆盖`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md)、
  `continues_with_allow_no_backup_when_protection_fails`（crates/lkit-cli/src/workflows/uninstall/mod.rs）
- 说明：Rust 层已覆盖保护备份失败后 `--allow-no-backup` 继续并记录 `no_backup: true`；
  Docker E2E 无卸载 `--allow-no-backup` 场景，卸载事务的 `no_backup` 标志无 CLI 层断言。

## UNI-06

**默认卸载删除 landscape 根受管内容，lkit 地盘保留 `config.toml`、`backups/` 与目录结构**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[卸载语义](../../../deployment/layout-and-state.md#卸载语义)、
  `uninstalls_none_mode_and_keeps_the_lkit_territory`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）、
  `uninstalls_an_existing_installation_through_full_cli`（crates/lkit-cli/tests/install_fixture_e2e/uninstall.rs）
- 说明：删除 landscape 根的 `releases/`、`data/`、`service/` 与 `current`；lkit 地盘
  `config.toml` 内容逐字节不变，保护 `.lkb` 保留在 `backups/`；本安装的事务与日志在
  卸载完成后清理（`transactions/`、`logs/` 目录本身保留），见[卸载语义](../../../deployment/layout-and-state.md#卸载语义)。

## UNI-07

**`--keep-data` 保留 landscape 根 `data/`，其余受管内容删除且安装视为已卸载**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#清理选项)、
  `keep_data_preserves_data_and_removes_the_rest`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）
- 说明：`install-state.json` 被删除；再次 `lkit install` 视为全新首次安装。

## UNI-08

**网络接管特征（宿主网络服务被 stop/disable/mask）警告后仍可继续卸载**

- 测试层：Rust workflow、CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#卸载前检查)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/uninstall.rs)
- 说明：检测接管特征（交互模式以确认提示呈现，确认后继续）不阻断；卸载后宿主网络
  服务保持现状，由用户自行恢复。

## UNI-09

**卸载中断后下次命令前向完成，不自动回滚**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[中断恢复](../../../deployment/transactions-and-recovery.md)、
  `recovers_interrupted_uninstall_by_forward_completion`（crates/lkit-cli/src/deployment/transaction/recovery.rs）
- 说明：在 `stopping` 或 `activating` 阶段中断（恢复共用同一前向完成分支），下次任意
  lkit 命令继续完成注销、删除与提交；`preparing` 阶段中断标记 `failed` 并保留现场。
- 缺口：恢复再次失败（标记 `failed` 并保留 `.lkb` 与事务现场）的分支无专门用例。

## UNI-10

**卸载成功后同一 landscape 根可再次执行全新首次安装**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md#执行与提交)、
  Docker E2E S7/S2（scripts/docker-e2e/run-scenarios.sh：卸载后安装 latest 根与 export 根）
- 说明：`install-state.json` 不存在后 `lkit install` 按 FirstInstall 处理并成功；卸载
  完成路径同时清理本安装的事务与日志。

## UNI-11

**未安装或状态损坏时拒绝卸载**

- 测试层：Rust workflow、CLI
- 状态：`已覆盖`
- 证据：[`lkit uninstall`](../../../commands/uninstall.md)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/uninstall.rs)
- 说明：无有效 state 返回参数错误 `2` 且不写任何文件；状态损坏按损坏判定阻断
  （退出码非 0），不触碰 config/安装现场。

## UNI-12

**卸载不影响 lkit 常驻 daemon；lkit 地盘与 daemon 生命周期分离**

- 测试层：Rust workflow、CLI E2E
- 状态：`部分覆盖`
- 证据：[`lkit self`](self.md)、[安装布局与状态](../../../deployment/layout-and-state.md)、
  `systemd_mode_unregisters_the_unit_but_not_lkit_daemon`（crates/lkit-cli/src/workflows/uninstall/cleanup.rs）
- 说明：daemon 已注册运行时卸载 landscape：`lkit.service` 保持注册与运行，pidfile 存活；
  daemon 恢复循环在无未完成事务时无动作。移除 daemon 使用 `lkit self remove`。
- 缺口：daemon **运行中**（pidfile 存活）执行卸载的 CLI 场景无直接断言；lkit 卸载
  （`self remove`）与 daemon 恢复目标从 lkit 地盘发现的断言。

## UNI-13

**控制台 Uninstall 面板确认、委托与结果展示（暂隐藏，代码保留）**

- 测试层：控制台集成测试
- 状态：`部分覆盖`
- 证据：[Ratatui 管理控制台](../../../interaction/console.md)
- 说明：面板当前从侧栏隐藏（`Menu::ALL` 中 `Self::Uninstall` 注释保留），只经 CLI
  `lkit uninstall` 使用；面板渲染、确认层与委托的 Rust 单测保留并通过，重新启用后
  需补充 TUI 端到端场景。确认层展示版本、数据损失范围、保留物（lkit 地盘）与网络
  接管警告；`--console-confirmed` 委托共享命令分发，systemd 模式经 worker 执行，成功
  展示备份 ID 与保留物清单。
