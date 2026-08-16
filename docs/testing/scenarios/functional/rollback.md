# 自动备份与回滚场景

## RB-01

**`.lkb` header、metadata、归档和 checksum 合法，包含项与排除项符合 minimal scope**

- 测试层：Rust 单元、Docker E2E
- 状态：`部分覆盖`
- 说明：格式和 metadata 已覆盖；Docker 尚未直接检查全部 tar 内容，见 [备份边界](../rollback.md#备份边界)。

## RB-02

**目标版本启动即退、健康失败、稳定期退出或启动超时后触发回滚**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[S3 失败启动矩阵](../extended.md#s3-失败启动矩阵)

## RB-03

**有 `.lkb` 时恢复旧版本、配置、服务状态和隔离的 resolv.conf**

- 测试层：Rust rollback workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[失败切换](../rollback.md)
- 说明：Rust 测试先污染现场旧 release 和 Geo，再断言 binary、static、Geo 与 init 配置
  均恢复为 `.lkb` 中的独特内容；Docker E2E 验证配置、服务状态与隔离的 resolv.conf。

## RB-04

**无备份切换失败时恢复 `current` 和服务事实，但不宣称恢复 data**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[停止服务后的切换](../../../commands/switch.md#停止服务后的切换)

## RB-05

**回滚成功返回退出码 `5`，不误报切换成功**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[退出码](../../../commands/output-and-exit-codes.md)

## RB-06

**自动回滚自身失败时保留可诊断事务、返回 `6` 并提示人工恢复**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[退出码](../../../commands/output-and-exit-codes.md)、`switch_rollback_failure_returns_rollback_failed_and_preserves_diagnostics`（crates/lkit-cli/src/workflows/install/switch_tests.rs）
- 说明：激活验证失败触发 `.lkb` 回滚后，回滚自身的健康检查也失败（探测恒失败），返回 `SwitchOutcome::RollbackFailed`（命令层映射退出码 `6`）；事务保持 `failed`，`failed-data`、`replaced-release`、解包 `restore` 目录与保护备份保留供人工恢复，`current` 已在健康检查前恢复。

## RB-07

**回滚或主机中断后，下次调用按事务 phase 幂等恢复**

- 测试层：Rust 事务测试
- 状态：`部分覆盖`
- 说明：多阶段恢复有低层测试，缺少完整 CLI 故障现场；见 [事务恢复](../../../deployment/transactions-and-recovery.md)。
