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

**新仓库为同版本提供不同资产时拒绝 repair**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 说明：[`lkit repair`](../../../commands/repair.md)

## REP-04

**binary repair 激活失败后使用 `.lkb` 和旧二进制回滚**

- 测试层：Rust workflow
- 状态：`部分覆盖`
- 说明：回滚实现存在，缺少完整 CLI/Docker 故障场景；见 [Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)。

## REP-05

**binary 或 static repair 的恢复动作也失败时返回人工恢复结果**

- 测试层：CLI/Docker E2E
- 状态：`待补充`
- 缺口：需要独立故障注入场景。

## REP-06

**systemd 下 repair 重建后端并完成完整健康检查**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)
