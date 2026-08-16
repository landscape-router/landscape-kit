# 安全与环境检查场景

## SEC-01

**安装根、受管路径和压缩包中的符号链接或路径逃逸被拒绝**

- 测试层：Rust 安全测试
- 状态：`已覆盖`
- 证据：[验收标准](../../../acceptance.md#安装状态与路径)

## SEC-02

**密码、API token、Authorization 和敏感 URL 不进入输出或事务日志**

- 测试层：Rust 单元、CLI E2E
- 状态：`部分覆盖`
- 说明：输入和日志过滤分别覆盖（Debug/CLI 参数脱敏、token 校验、事务日志仅 phase 行）；
  缺少统一端到端日志扫描（Docker E2E 已有固定口令与 fixture token，可对
  `territory/logs/` 与命令输出做统一扫描断言）；见 [输出约束](../../../commands/output-and-exit-codes.md)。

## SEC-03

**成功、普通失败、用法错误、回滚成功和回滚失败分别返回 `0/1/2/5/6`**

- 测试层：Rust/CLI/Docker E2E
- 状态：`部分覆盖`
- 说明：`0/1/2/5` 分层覆盖；`6` 的实现存在于各命令（repair/switch/restore/migrate/
  reinit 的 `RollbackFailed` 分支均映射退出码 6），但只有 network 有直接断言
  （`network_rollback_failure_preserves_scene_and_marks_transaction_failed`），
  其余命令的 `6` 缺 CLI 层断言；见 [退出码](../../../commands/output-and-exit-codes.md)。

## ENV-01

**`lkit check` 在支持环境报告检查结果且全过程只读**

- 测试层：Rust 单元、实际主机验收
- 状态：`部分覆盖`
- 说明：检查项逻辑已有测试，完整宿主环境由低频验收承担；见 [`lkit check`](../../../check.md)。

## ENV-02

**不支持的平台、权限、内核、依赖或端口冲突产生正确阻断级别**

- 测试层：Rust 单元
- 状态：`部分覆盖`
- 证据：[check 验收标准](../../../check.md#验收标准)
- 缺口：端口冲突和状态聚合已有直接测试；平台、root、内核能力、依赖缺失及 CLI
  退出码仍缺少可控场景测试。

## ENV-03

**发行版 ID 不作为安装白名单，依赖错误按包管理器提供安装建议**

- 测试层：Rust 单元、Shell 安装器测试
- 状态：`部分覆盖`
- 证据：[check 适用范围](../../../check.md#适用范围)、[安装入口](../../../release/lkit.md#安装入口)
- 缺口：Fedora、Arch Linux 和 openSUSE 的完整宿主 preflight 仍需低频 VM smoke。
