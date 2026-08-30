# 修复场景

## REP-01

**`repair binary` 从可信仓库恢复被篡改后端并保持版本不变**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S1 repair](../extended.md#s1-repair-全流程)

## REP-02

**`repair static` 恢复发布版页面且不停止服务、不创建 `.lkb`**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S1 repair](../extended.md#s1-repair-全流程)
- 说明：static 通过原子目录替换热更新；Rust workflow 断言不创建 backups，Docker E2E
  断言 repair 前后 MainPID 和 `.lkb` 数量均不变。

## REP-03

**`repair static` 在记录身份与仓库不一致时仍以仓库为准恢复官方页面**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[`lkit repair`](../../../commands/repair.md)、`repairs_static_from_a_repository_with_different_assets`（lkit-cli/src/workflows/repair.rs）
- 说明：state 记录的 static 身份与仓库 manifest 不一致时（如恢复备份后），repair
  不再拒绝，而是下载校验官方包、替换静态目录、更新 state 身份并刷新版本目录
  `static.zip`。自定义前端源激活时 repair 恢复自定义前端（`--official` 强制官方），
  见 [FE-06](frontend.md#fe-06)。

## REP-04

**binary repair 激活失败后使用 `.lkb` 和旧二进制回滚**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)、`binary_repair_rolls_back_when_activation_fails`（lkit-cli/src/workflows/repair.rs）
- 说明：以落盘二进制作为阶段信号的探测在激活验证期间失败（二进制已被可信内容替换）、回滚从 `.lkb` 重建后恢复为漂移内容时通过；断言 `RepairOutcome::RolledBack`、事务 `rolled_back`、二进制与 state 摘要恢复为修复前内容、`current` 不变。

## REP-05

**binary 或 static repair 的恢复动作也失败时返回人工恢复结果**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：`binary_repair_rollback_failure_returns_rollback_failed`（lkit-cli/src/workflows/repair.rs）
- 说明：探测恒失败时激活失败触发回滚，回滚自身的健康检查也失败，返回 `RepairOutcome::RollbackFailed`（命令层映射退出码 `6`）；事务保持 `failed`，`failed-data`、`replaced-release` 与 `repaired-binary` 诊断现场保留供人工恢复。

## REP-06

**systemd 下 repair 重建后端并完成完整健康检查**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)
