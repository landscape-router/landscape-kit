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
- 证据：[退出码](../../../commands/output-and-exit-codes.md)、`switch_rollback_failure_returns_rollback_failed_and_preserves_diagnostics`（lkit-cli/src/workflows/install/switch_tests.rs）
- 说明：激活验证失败触发 `.lkb` 回滚后，回滚自身的健康检查也失败（探测恒失败），返回 `SwitchOutcome::RollbackFailed`（命令层映射退出码 `6`）；事务保持 `failed`，`failed-data`、`replaced-release`、解包 `restore` 目录与保护备份保留供人工恢复，`current` 已在健康检查前恢复。

## RB-07

**回滚或主机中断后，下次调用按事务 phase 幂等恢复**

- 测试层：Rust 事务测试、Docker E2E
- 状态：`已覆盖`
- 证据：[事务恢复](../../../deployment/transactions-and-recovery.md)、
  Docker E2E S8（确定性 preparing 现场后 `lkit switch` 恢复并标记 failed）与
  S12（restore verifying 阶段 kill 后 `lkit reconcile` 幂等恢复并还原 `data`），
  scripts/docker/lifecycle/run-scenarios.sh

## RB-08

**安装根与 lkit 地盘处于不同文件系统时，修复、恢复和失败回滚保持可用**

- 测试层：Rust 隔离单元、CI Fixture E2E
- 状态：`已覆盖`
- 证据：[工作目录校验](../../../../lkit-cli/src/deployment/transaction/workspace.rs)、
  [current 恢复](../../../../lkit-cli/src/backup/rollback.rs)、
  [修复成功](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)、
  [restore 中断恢复](../../../../lkit-cli/tests/install_fixture_e2e/restore.rs)、
  [switch 恢复失败后重试](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)、
  [reinit 确认](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)。
- 说明：CI 把地盘放在临时 tmpfs 目录、安装根放在普通测试磁盘，并断言二者设备 ID
  不同；不把所有路径放在同一个临时目录来代替跨文件系统验证。验证静态和后端修复
  成功、restore 激活后崩溃恢复旧数据及提交回执、switch 恢复暂时失败后再次恢复，
  以及 reinit 进入确认并提交。单元覆盖工作目录身份、权限、软链接拒绝、字段缺失的
  事务兼容读取，以及 current 恢复不依赖地盘且保留无关临时链接。系统场景只由 CI 执行。
