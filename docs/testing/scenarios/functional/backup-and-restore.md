# 备份与恢复

## BKP-01

**运行中的 systemd 实例创建手工 minimal 备份**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[backup 命令](../../../commands/backup.md)、[`.lkb` 规格](../../../backup/lkb-and-rollback.md)、[S10 手工备份与恢复](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：通过导出 API 创建 `auto: false` 备份，断言版本、架构、remark、归档内容（含 `static.zip`）和 `0600` 权限。

## BKP-02

**none manager 外部实例创建备份，导出失败时不留下最终文件**

- 测试层：CLI fixture E2E、Rust 命令层
- 状态：`已覆盖`
- 证据：[backup create](../../../commands/backup.md#backup-create)、`create_writes_manual_backup_without_any_service_manager`、`create_export_failure_leaves_no_final_file`（crates/lkit-cli/src/commands/backup.rs）、`read_api_token` 单测（crates/lkit-cli/src/backup/export.rs）
- 说明：none 模式命令层测试完整走 `backup create` 流程（导出 API、token、归档自校验），断言归档内容为导出配置而非种子文件；导出返回 500 时 `backups/` 不留任何 `.lkb`。

## BKP-03

**列出、查看和验证内部或外部 `.lkb`**

- 测试层：CLI fixture E2E、Rust 单元、Docker E2E
- 状态：`已覆盖`
- 证据：[backup list、show 与 verify](../../../commands/backup.md#backup-list)、[容器格式](../../../backup/lkb-and-rollback.md#容器格式-v1)、`list_marks_symlinks_and_unsafe_permissions_invalid`、`list_marks_content_incomplete_backups_invalid`（crates/lkit-cli/src/commands/backup.rs）、Docker E2E S10
- 说明：验证损坏 checksum、header、路径逃逸、符号链接和权限不安全文件不会被误报为有效；Docker E2E S10 覆盖 list/show/verify 成功路径。

## BKP-04

**备份输出原子写入且拒绝覆盖既有文件**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[创建顺序与存放](../../../backup/lkb-and-rollback.md#创建顺序与存放)、`output_refuses_existing_files_and_symlinks`、`publish_no_replace_never_overwrites_an_existing_target`
- 说明：`--output` 拒绝已存在文件和符号链接，写入权限为 `0600`；`.lkb` 同名不覆盖由容器层保证。

## BKP-05

**手工备份备注：`--remark`、交互输入与校验**

- 测试层：Rust 单元（pty）
- 状态：`部分覆盖`
- 证据：[backup create](../../../commands/backup.md#backup-create)、`remark_resolution_uses_flag_or_empty_default`、`rejects_invalid_remarks`（crates/lkit-cli/src/backup/lkb.rs）
- 说明：`--remark` 优先；非交互或无法打开终端时缺省为空；超过 256 字符或含控制字符时返回参数错误 `2`。
- 缺口：交互模式经 `/dev/tty` 提示输入的 pty 路径未覆盖（仓库无 pty 测试设施）。

## BKP-06

**`verify` 拒绝缺失必需条目的残缺备份**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[backup show 与 backup verify](../../../commands/backup.md#backup-show-与-backup-verify)、`rejects_backups_missing_required_entries`、`rejects_directory_entry_named_static_zip`（crates/lkit-cli/src/backup/lkb.rs）、`verify_cleans_up_temp_dirs_and_rejects_incomplete_archives`（crates/lkit-cli/src/commands/backup.rs）、`creates_verifies_and_extracts_backup`（解包目录/文件权限断言）
- 说明：归档缺少 `landscape-webserver`、`landscape_init.toml`、`static.zip`（或非普通文件）、`static`、`geo_tmp` 任一必需条目时 verify 返回失败；解包目录与文件分别保持 `0700`/`0600`，verify 临时目录（uuid 命名）在成功与失败路径都不留残留。

## BKP-07

**`list` 对权限/所有者不安全与符号链接条目显示 invalid 并返回失败**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[backup list](../../../commands/backup.md#backup-list)、`list_marks_symlinks_and_unsafe_permissions_invalid`（crates/lkit-cli/src/commands/backup.rs）
- 说明：权限宽于 `0600`、非 root 所有以及符号链接形式的 `.lkb` 条目标记 invalid 并计入失败；符号链接不被跟随。

## RST-01

**从 `.lkb` 恢复同版本现有 systemd 安装**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[restore 命令](../../../commands/restore.md)、[手工 restore](../../../backup/lkb-and-rollback.md#手工-restore)、[S10 手工备份与恢复](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：恢复前自动创建保护备份；目标 release、空 data、初始化配置、服务身份和健康检查全部成功后提交；state 的 `static_archive` 身份与备份内压缩包一致，`config.toml` 中的来源记录不变。

## RST-02

**恢复到较低或较高版本，不受 switch 升级方向限制**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[restore 命令](../../../commands/restore.md)、`restores_cross_version_without_systemd`（crates/lkit-cli/src/workflows/restore.rs）、[S13 systemd 跨版本 restore](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：Rust 工作流测试覆盖无 systemd 跨版本恢复；Docker E2E S13 在 systemd 模式用早期版本备份降级恢复（5.0.0 → 2.0.0），断言事务 `from_version`/`target_version`、保护备份、`config.toml` 来源记录不变，恢复后 state 资产身份与备份内容一致。

## RST-03

**目标恢复激活或健康检查失败时自动恢复原状态并返回 `5`**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)、[S11 restore 激活失败自动回滚](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：用 `delayed-ready` 版本（启动延迟 2500ms）创建备份，restore 改用 2000ms 启动超时的运行时，激活超时失败后 systemd 模式内联回滚：原 `current`、state、unit、enabled/active、resolv.conf 和数据全部恢复，事务标记 `rolled_back`，返回退出码 `5`；保护备份仍被创建并记录。

## RST-04

**保护备份失败时默认阻断，显式 `--allow-no-backup` 才能继续**

- 测试层：CLI fixture E2E、Rust workflow
- 状态：`已覆盖`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)、`restore_blocks_without_allow_no_backup_when_protection_fails`、`restore_continues_with_allow_no_backup_when_protection_fails`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：导出失败时默认阻断且现场不变；`--allow-no-backup` 继续时事务记录 `no_backup: true`、`backup: null`，restore 正常提交。

## RST-05

**restore 中断后按 phase 恢复，回滚失败返回 `6` 并保留诊断现场**

- 测试层：Rust 事务测试、Docker E2E
- 状态：`已覆盖`
- 证据：[事务中断恢复](../../../deployment/transactions-and-recovery.md#中断恢复)、[S12 restore 中断后 phase 恢复](../../../../scripts/docker-e2e/run-scenarios.sh)、`rollback_restores_previous_data_from_transaction_dir`、`rollback_treats_consumed_previous_data_as_already_restored`、`rollback_failure_marks_the_transaction_failed`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：Docker E2E 在目标激活 `verifying` 阶段 kill 掉 `lkit`，断言事务停在非终结阶段且 data 已移入 `previous-data`，下次命令经恢复入口完整回滚（`rolled_back`、data/current/state 恢复）；Rust 测试覆盖回滚失败标记 `failed`（对应退出码 `6`）与 previous-data 幂等恢复。

## RST-06

**损坏、架构不匹配或缺少必要内容的备份在停止服务前拒绝**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)、[BackupMetadata Schema v1](../../../backup/lkb-and-rollback.md#backupmetadata-schema-v1)、`rejects_tampered_files`、`rejects_escaping_tar_entries`、`rejects_backups_missing_required_entries`（crates/lkit-cli/src/backup/lkb.rs）
- 说明：损坏 checksum、header、路径逃逸、符号链接和权限不安全文件不会被误报为有效；架构不匹配与归档缺少必要条目时拒绝，且不停止服务、不改变 current。

## RST-07

**none manager 恢复要求外部实例停止并提交 pending/未验证状态**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[激活与提交](../../../commands/restore.md#激活与提交)、`restores_cross_version_without_systemd`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：Rust 工作流测试断言提交 `initialization.status: pending`、`service.verified: false`。

## RST-08

**确认拒绝或缺 `--yes` 时不创建事务、不改变现场**

- 测试层：Rust 工作流、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)、[事务 schema](../../../deployment/transactions-and-recovery.md)、`restore_requires_yes_in_non_interactive_mode`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：非交互缺 `--yes` 返回参数错误 `2`；`transactions/` 无新增文件，`--file` 不产生暂存拷贝，服务与 current 不变。

## RST-09

**none 模式激活后失败不内联回滚，由下次命令按 phase 恢复**

- 测试层：Rust 工作流、Docker E2E
- 状态：`已覆盖`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)、`none_mode_activation_failure_is_recovered_by_next_command`（crates/lkit-cli/src/workflows/restore.rs）、[S12 restore 中断后 phase 恢复](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：none 模式激活失败返回普通失败且不内联回滚，事务停在 `activating`，`previous-data` 保留在事务目录；现场修复后经 `recover_interrupted` 恢复入口恢复原 data、current 与 state，事务标记 `rolled_back`。

## RST-10

**非法 `--backup` ID 与 `static.zip` 非普通文件条目被拒绝**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`lkit restore` 命令格式](../../../commands/restore.md)、[`.lkb` 校验](../../../backup/lkb-and-rollback.md)、`rejects_malformed_backup_ids_before_creating_a_transaction`（crates/lkit-cli/src/workflows/restore.rs）、`rejects_directory_entry_named_static_zip`（crates/lkit-cli/src/backup/lkb.rs）
- 说明：`--backup` 非 `YYYYMMDD-HHMMSS-8hex` 格式返回参数错误 `2` 且不创建事务；`static.zip` 为目录条目时解包校验拒绝，restore 不会继续。

## RST-11

**none 模式非交互 `--yes` 完成恢复，不触发 TTY 确认**

- 测试层：Rust 工作流
- 状态：`已覆盖`
- 证据：[`lkit restore`](../../../commands/restore.md)、`none_mode_proceeds_with_non_interactive_yes`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：`--non-interactive --yes` 时"外部实例已停止"确认以 `--yes` 代替，不再调用 `/dev/tty`；none 模式恢复提交 pending/未验证状态。

## RST-12

**同版本 restore 失败回滚恢复原 release 内容**

- 测试层：Rust 工作流
- 状态：`已覆盖`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)、`same_version_rollback_restores_the_original_release`（crates/lkit-cli/src/workflows/restore.rs）
- 说明：同版本 restore 激活失败时，被移入事务目录 `replaced-release` 的原 release 被移回；回滚后磁盘二进制/静态资源与回滚前一致，`verify_current_backend` 通过。

## RST-13

**控制台发起的 restore 以 TUI 确认为唯一确认，不再出现 `/dev/tty` 二次确认**

- 测试层：Rust workflow、Rust 单元（console）
- 状态：`已覆盖`
- 证据：[交互控制台](../../../interaction/console.md)、`console_confirmed_skips_interactive_confirmations`（crates/lkit-cli/src/workflows/restore.rs）、`backup_restore_flow_builds_restore_command`（crates/lkit-cli/src/console.rs，断言 `console_confirmed` 与 `--console-confirmed` 参数）
- 说明：交互模式下 `console_confirmed` 使恢复工作流跳过恢复计划与 minimal scope 的 tty 确认（worker 是独立进程，无法读取 TUI 输入，继续交互确认会阻塞），恢复正常提交；控制台覆盖层同时展示 minimal scope 数据损失警告，`--backup`/`--yes`/`--console-confirmed` 均传入分发参数。
- 缺口：真实 systemd worker 内的无 tty 阻塞路径未单独做 E2E 断言（Docker E2E 恢复用例均带 `--non-interactive`）。

## BKP-08

**控制台创建备份对话框：备注输入、校验与分发**

- 测试层：Rust 单元（console）
- 状态：`已覆盖`
- 证据：[交互控制台](../../../interaction/console.md)、`backup_create_action_builds_cli_and_domain_request`（crates/lkit-cli/src/console.rs，断言创建对话框渲染、备注逐字符输入、`--remark` 与结构化请求一致）
- 说明：Enter 打开创建对话框（标题、minimal scope 说明、备注输入行带光标、Enter 创建/Esc 取消）；最多 256 字符、Enter 提交走与 CLI 相同的备注校验，空备注直接创建不带 `--remark`。

## BKP-09

**switch、repair 与 restore 的保护快照带固定备注**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[`.lkb` 备份与回滚](../../../backup/lkb-and-rollback.md)、`restores_cross_version_without_systemd`（断言恢复后保护备份存在，crates/lkit-cli/src/workflows/restore.rs）、Docker E2E S10/S11/S13（断言 restore 创建保护备份并记录在事务中）
- 说明：保护快照 `auto: true` 且备注为固定本地化文案（`switch 前自动备份`、`repair 前自动备份`、`restore 前自动保护备份`）；Docker E2E 已断言保护备份存在，备注值由 Rust 工作流直接构造时以 `tr!` 生成。
- 缺口：Docker E2E 未断言保护备份的 remark 具体值（仅断言存在）。

## RST-14

**委托 restore 全屏页显示操作标题与步骤进度条**

- 测试层：Rust 单元（presentation）、Rust workflow
- 状态：`已覆盖`
- 证据：[激活与提交](../../../commands/restore.md)、`renders_step_progress_gauge_for_stepped_operations`（crates/lkit-cli/src/interaction/presentation.rs，断言操作标题与 `2/4` 步骤 Gauge）、restore 工作流在准备/停止服务/激活/验证阶段发送 `operation_progress` 事件（crates/lkit-cli/src/workflows/restore.rs）、`operation_title` 按子命令生成标题（crates/lkit-cli/src/systemd_worker.rs）
- 说明：restore 不发字节下载进度，全屏页按 systemd 4 步（准备 1/4 → 停止服务 2/4 → 激活 3/4 → 初始化与健康检查 4/4）渲染步骤 Gauge，标题为"正在恢复 Landscape"；install 的字节进度条不受影响。
- 缺口：真实 worker 进程到全屏页的事件链路未单独 E2E 断言（Docker E2E 在无控制台终端下运行）。
