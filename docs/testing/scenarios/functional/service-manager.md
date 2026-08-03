# Service Manager 迁移场景

## SM-01

**systemd → none 停止并注销受管服务，保持 Landscape 停止**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[迁移语义](../../../commands/service-manager.md#systemd--none)

## SM-02

**none → systemd 注册、启动并验证服务**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S6 服务管理器迁移](../extended.md#s6-服务管理器迁移none--systemd)

## SM-03

**none → systemd 接管 pending 初始化并提交 complete**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S6 服务管理器迁移](../extended.md#s6-服务管理器迁移none--systemd)

## SM-04

**目标 manager 与当前相同时幂等成功且不创建迁移事务**

- 测试层：CLI/workflow
- 状态：`待补充`
- 说明：行为已定义，缺少直接场景断言；见 [`lkit service-manager`](../../../commands/service-manager.md)。

## SM-05

**用户拒绝、无 `/dev/tty` 或固定端口被占用时不接管 systemd**

- 测试层：Rust 交互与 preflight 测试
- 状态：`部分覆盖`
- 说明：各保护逻辑分层覆盖，缺少统一 CLI 场景。

## SM-06

**foreign unit 所有权冲突时阻断接管并保持已提交 manager**

- 测试层：Rust 所有权与 workflow 测试
- 状态：`部分覆盖`
- 说明：所有权判定已有低层测试，缺少 Docker CLI 场景；真实 manager 下的清理另见 [SYS-04](../systemd-smoke.md#sys-04)。

## SM-07

**迁移中途失败时恢复原 service-manager 事实；恢复失败时要求人工介入**

- 测试层：Rust workflow/CLI E2E
- 状态：`待补充`
- 说明：成功路径已有覆盖，失败恢复缺少直接故障场景。
