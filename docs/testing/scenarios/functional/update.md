# 版本更新场景

`lkit update` 是 [`lkit switch`](../../../commands/switch.md) 的交互式薄封装：选择读取渠道、
解析目标版本、要求用户确认后复用 switch 流水线执行。渠道选择、确认与零副作用拒绝是
update 独有的行为；事务、备份、回滚与退出码语义全部继承 switch 域（见
[switch.md](switch.md) 与 [rollback.md](rollback.md)）。

## UP-01

**交互选择渠道后展示当前与目标版本**

- 测试层：CLI/伪终端
- 状态：`已覆盖`
- 证据：[`lkit update`](../../../commands/update.md)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：未显式 `--repository` 时通过 `/dev/tty` 选择渠道；首个 state 来源选项支持直接回车，
  列表同时显示 GitHub、Mirror 和自定义 HTTP。解析 `latest` 后展示 `当前 <X> → 目标 <Y>`。

## UP-02

**默认 latest 确认后成功升级**

- 测试层：Docker E2E
- 状态：`待补充`
- 证据：[成功切换](../lifecycle.md#成功切换-200)
- 说明：确认后走 switch 提交路径，事务 `operation` 为 `switch`，`.lkb` 与 systemd
  激活语义与 `lkit switch --version latest` 一致。

## UP-03

**`--version` 固定目标确认后升级**

- 测试层：Docker E2E
- 状态：`待补充`
- 说明：显式版本与默认 latest 在目标比较、确认和执行阶段遵循相同流程；降级行为继承
  [`SW-02`](switch.md#sw-02)，同版本行为由 `UP-04` 单独定义。

## UP-04

**已是最新时不变更并返回 `0`**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：目标与 `active_version` 相同时输出已是最新，返回 `0`，不创建事务、不下载资产，
  也不验证或持久化所选仓库来源；与 switch 进入同版本安装校验不同。

## UP-05

**拒绝确认时零副作用**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：确认在创建事务与备份之前；拒绝时退出码 `1`，不创建事务、不下载目标资产，
  `current`、state 与既有事务不变。

## UP-06

**非交互环境报错并提示使用 switch**

- 测试层：CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：无 `/dev/tty` 或 `--non-interactive` 时返回普通失败（退出码 `1`），提示改用
  `lkit switch --version <VERSION>`；需要时再追加 `--repository <BASE_URL>`。

## UP-07

**升级失败自动回滚（继承 switch 语义）**

- 测试层：Docker E2E
- 状态：`部分覆盖`
- 证据：[失败切换与自动回滚](../rollback.md)、[S2/S3 扩展场景](../extended.md)
- 缺口：现有回滚证据针对 `lkit switch`，update 委托同一流水线，无独立回滚路径。
