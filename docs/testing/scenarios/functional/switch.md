# 版本切换场景

`lkit switch` 只允许向更高的 stable 版本升级。较低版本必须在改变安装前拒绝；相同版本
沿用同版本安装校验，不创建版本切换事务。

## SW-01

**运行中的 systemd 安装成功升级到更高版本**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[成功切换](../lifecycle.md#成功切换-200)

## SW-02

**显式切换到较低的历史 stable 版本时拒绝且不改变安装**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[版本切换工作流](../../../../crates/lkit-cli/src/workflows/install.rs)、[`lkit switch`](../../../commands/switch.md)
- 说明：使用 `0.22.2 → 0.21.1` 断言返回参数用法错误，`current`、state 和既有事务不变，
  且不下载目标二进制或静态资产。

## SW-03

**目标版本已经 active 时拒绝创建无意义事务**

- 测试层：Rust workflow/CLI
- 状态：`待补充`
- 说明：实现已有保护，缺少直接场景断言。

## SW-04

**none manager 下只切换文件和状态，不执行运行态检查**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[无 systemd 激活语义](../../../workflows/lifecycle.md#6-激活)

## SW-05

**初始化完成后现场 init 文件变化或删除不阻断切换且不会被改写**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[初始化与凭据](../../../interaction/initialization-and-credentials.md#保留与变更)

## SW-06

**显式仓库覆盖无需二次确认，且同版本覆盖的资产必须一致**

- 测试层：Rust workflow、repair workflow
- 状态：`部分覆盖`
- 证据：[仓库来源变化策略](../../../release/source-policy.md)
- 缺口：不同目标版本的显式仓库覆盖和 repair 的资产不一致已有直接测试；未切换版本时
  只更新仓库来源的完整 same-version 路径仍缺直接测试。

## SW-07

**配置导出失败时不停止服务、不创建备份并保持旧版本**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[S2 导出失败](../extended.md#s2-导出失败回滚export_error)

## SW-08

**服务已停止时默认拒绝切换且不改变现有状态**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S4 停止服务后切换](../extended.md#s4-停止服务后切换)

## SW-09

**服务已停止且显式允许时无备份切换成功**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[S4 停止服务后切换](../extended.md#s4-停止服务后切换)

## SW-10

**服务仍运行时忽略 `--allow-no-backup`、给出警告并照常创建 `.lkb`**

- 测试层：CLI/Docker E2E
- 状态：`待补充`
- 说明：行为已定义但当前场景未直接断言输出和备份；见 [`lkit switch`](../../../commands/switch.md#停止服务后的切换)。
