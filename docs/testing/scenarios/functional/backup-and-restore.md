# 备份与恢复

## BKP-01

**运行中的 systemd 实例创建手工 minimal 备份**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[backup 命令](../../../commands/backup.md)、[`.lkb` 规格](../../../backup/lkb-and-rollback.md)、[S10 手工备份与恢复](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：通过导出 API 创建 `auto: false` 备份，断言版本、架构、remark、归档内容（含 `static.zip`）和 `0600` 权限。

## BKP-02

**none manager 外部实例创建备份，导出失败时不留下最终文件**

- 测试层：CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[backup create](../../../commands/backup.md#backup-create)
- 缺口：导出失败不留文件已由切换失败路径（S2）间接证明；none manager 手工备份创建与 token 不安全路径未覆盖。

## BKP-03

**列出、查看和验证内部或外部 `.lkb`**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`部分覆盖`
- 证据：[backup list、show 与 verify](../../../commands/backup.md#backup-list)、[容器格式](../../../backup/lkb-and-rollback.md#容器格式-v1)
- 说明：验证损坏 checksum、header、路径逃逸、符号链接和权限不安全文件不会被误报为有效；Docker E2E S10 覆盖 list/show/verify 成功路径。
- 缺口：`list` 对权限过宽/非 root 所有文件的 invalid 标记，以及 verify 对残缺归档（缺二进制/init/static 等必需条目）的拒绝尚未自动化。

## BKP-04

**备份输出原子写入且拒绝覆盖既有文件**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[创建顺序与存放](../../../backup/lkb-and-rollback.md#创建顺序与存放)
- 说明：`--output` 拒绝已存在文件和符号链接，写入权限为 `0600`；`.lkb` 同名不覆盖由容器层保证。

## BKP-05

**手工备份备注：`--remark`、交互输入与校验**

- 测试层：Rust 单元（pty）
- 状态：`部分覆盖`
- 证据：[backup create](../../../commands/backup.md#backup-create)
- 说明：`--remark` 优先；未提供时交互模式通过 `/dev/tty` 提示输入（空回车 = 无备注），非交互模式缺省为空；超过 256 字符或含控制字符时返回参数错误 `2`。
- 缺口：交互 pty 路径与非法备注拒绝路径未覆盖。

## BKP-06

**`verify` 拒绝缺失必需条目的残缺备份**

- 测试层：Rust 单元
- 状态：`待补充`
- 证据：[backup show 与 backup verify](../../../commands/backup.md#backup-show-与-backup-verify)
- 说明：归档缺少 `landscape-webserver`、`landscape_init.toml`、`static.zip`（或非普通文件）、`static/`、`geo_tmp/` 任一必需条目时 verify 返回失败；解包目录与文件分别保持 `0700`/`0600`，verify 临时目录不可预测。

## BKP-07

**`list` 对权限/所有者不安全与符号链接条目显示 invalid 并返回失败**

- 测试层：Rust 单元
- 状态：`待补充`
- 证据：[backup list](../../../commands/backup.md#backup-list)
- 说明：权限宽于 `0600`、非 root 所有以及符号链接形式的 `.lkb` 条目标记 invalid 并计入失败；符号链接不被跟随。

## RST-01

**从 `.lkb` 恢复同版本现有 systemd 安装**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[restore 命令](../../../commands/restore.md)、[手工 restore](../../../backup/lkb-and-rollback.md#手工-restore)、[S10 手工备份与恢复](../../../../scripts/docker-e2e/run-scenarios.sh)
- 说明：恢复前自动创建保护备份；目标 release、空 data、初始化配置、服务身份和健康检查全部成功后提交；state 的 `static_archive` 身份与备份内压缩包一致、`repository` 不变。

## RST-02

**恢复到较低或较高版本，不受 switch 升级方向限制**

- 测试层：Rust workflow、Docker E2E
- 状态：`部分覆盖`
- 证据：[restore 命令](../../../commands/restore.md)
- 说明：Rust 工作流测试覆盖无 systemd 跨版本恢复；版本来自备份 metadata，不下载仓库资产；state 的 `repository` 沿用当前安装，`static_archive` 身份从备份内 `static.zip` 现场计算。
- 缺口：systemd 模式跨版本恢复的 Docker E2E 未覆盖。

## RST-03

**目标恢复激活或健康检查失败时自动恢复原状态并返回 `5`**

- 测试层：Docker E2E
- 状态：`待补充`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)
- 说明：原 `current`、state、unit、enabled/active、resolv.conf 和事务前 data 恢复；目标现场保留。

## RST-04

**保护备份失败时默认阻断，显式 `--allow-no-backup` 才能继续**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)
- 说明：非交互模式还必须提供 `--yes`；用户拒绝不停止服务、不改变 current。
- 缺口：`--allow-no-backup` 继续路径未覆盖。

## RST-05

**restore 中断后按 phase 恢复，回滚失败返回 `6` 并保留诊断现场**

- 测试层：Rust 事务测试、CLI/Docker E2E、systemd-nspawn smoke
- 状态：`待补充`
- 证据：[事务中断恢复](../../../deployment/transactions-and-recovery.md#中断恢复)

## RST-06

**损坏、架构不匹配或缺少必要内容的备份在停止服务前拒绝**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)、[BackupMetadata Schema v1](../../../backup/lkb-and-rollback.md#backupmetadata-schema-v1)
- 说明：损坏 checksum、header、路径逃逸、符号链接和权限不安全文件不会被误报为有效；架构不匹配与归档缺少 `static.zip` 等必要条目时拒绝，且不停止服务、不改变 current。

## RST-07

**none manager 恢复要求外部实例停止并提交 pending/未验证状态**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[激活与提交](../../../commands/restore.md#激活与提交)
- 说明：Rust 工作流测试断言提交 `initialization.status: pending`、`service.verified: false`。

## RST-08

**确认拒绝或缺 `--yes` 时不创建事务、不改变现场**

- 测试层：Rust 工作流、CLI fixture E2E
- 状态：`待补充`
- 证据：[恢复前检查](../../../commands/restore.md#恢复前检查)、[事务 schema](../../../deployment/transactions-and-recovery.md)
- 说明：交互拒绝返回 `1`、非交互缺 `--yes` 返回参数错误 `2`；`transactions/` 无新增文件，`--file` 不产生暂存拷贝，服务与 current 不变。

## RST-09

**none 模式激活后失败不内联回滚，由下次命令按 phase 恢复**

- 测试层：Rust 工作流
- 状态：`待补充`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)
- 说明：返回普通失败 `1`；`previous-data`、previous-state 与保护备份保留在事务目录；下次命令经 `activating`/`verifying` 恢复入口恢复原 data、current 与 state。

## RST-10

**非法 `--backup` ID 与 `static.zip` 非普通文件条目被拒绝**

- 测试层：Rust 单元
- 状态：`待补充`
- 证据：[`lkit restore` 命令格式](../../../commands/restore.md)、[`.lkb` 校验](../../../backup/lkb-and-rollback.md)
- 说明：`--backup` 非 `YYYYMMDD-HHMMSS-8hex` 格式返回参数错误 `2`；`static.zip` 为目录条目时解包校验拒绝，restore 不会继续。

## RST-11

**none 模式非交互 `--yes` 完成恢复，不触发 TTY 确认**

- 测试层：Rust 工作流
- 状态：`待补充`
- 证据：[`lkit restore`](../../../commands/restore.md)
- 说明：`--non-interactive --yes` 时"外部实例已停止"确认以 `--yes` 代替，不再调用 `/dev/tty`；none 模式恢复提交 pending/未验证状态。

## RST-12

**同版本 restore 失败回滚恢复原 release 内容**

- 测试层：Rust 工作流
- 状态：`待补充`
- 证据：[失败与恢复](../../../commands/restore.md#失败与恢复)
- 说明：同版本 restore 激活失败时，被移入事务目录 `replaced-release` 的原 release 被移回；回滚后磁盘二进制/静态资源与回滚前一致，`verify_current_backend` 通过。
