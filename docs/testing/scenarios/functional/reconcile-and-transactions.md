# Reconcile 与事务场景

## REC-01

**初始化完成后忽略现场 init 文件内容变化并保持文件原样**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[S9 reconcile](../extended.md#s9-reconcile)

## REC-02

**接受受管 unit 内容变化并更新 service metadata**

- 测试层：Rust workflow/CLI E2E
- 状态：`部分覆盖`
- 说明：变化检测逻辑存在，Docker 场景尚未覆盖；见 [`lkit reconcile`](../../../commands/reconcile.md)。

## REC-03

**显式仓库覆盖无需二次确认且验证活动版本资产身份**

- 测试层：Rust workflow/CLI E2E
- 状态：`部分覆盖`
- 说明：仓库校验逻辑已有测试，缺少 reconcile CLI 场景。

## REC-04

**无变化时 reconcile 幂等成功且无需确认**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[S9 reconcile](../extended.md#s9-reconcile)

## REC-05

**缺失或损坏 state、缺失或漂移的 `current` 不被猜测重建**

- 测试层：Rust state 测试、Docker E2E
- 状态：`已覆盖`
- 证据：[S9 reconcile](../extended.md#s9-reconcile)

## TX-01

**同一 canonical 安装根上的并发管理命令被非阻塞锁拒绝**

- 测试层：Rust lock 测试
- 状态：`已覆盖`
- 证据：[验收标准](../../../acceptance.md#安装状态与路径)

## TX-02

**锁文件残留但没有进程持锁时允许继续**

- 测试层：Rust lock 测试
- 状态：`已覆盖`
- 证据：[验收标准](../../../acceptance.md#安装状态与路径)

## TX-03

**install、switch、repair 和 migration 的未完成事务按 phase 恢复**

- 测试层：Rust 事务测试、Docker E2E
- 状态：`部分覆盖`
- 说明：preparing switch 有 Docker 场景，其余主要为低层测试；见 [S8](../extended.md#s8-中断事务恢复)。

## TX-04

**损坏或路径不安全的 state/transaction 被拒绝且不修改现场**

- 测试层：Rust state 与事务测试
- 状态：`已覆盖`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md)
