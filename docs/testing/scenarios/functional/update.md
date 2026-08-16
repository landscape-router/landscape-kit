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
- 说明：未显式 `--repository` 时通过 `/dev/tty` 选择渠道；预置 `config.toml` 时
  首个"当前来源"选项支持直接回车（update 不修改该文件），文件不存在时选项从官方
  GitHub 开始（默认选中）；列表同时显示 GitHub、Mirror 和自定义 HTTP。解析
  `latest` 后展示 `当前 <X> → 目标 <Y>`。

## UP-02

**默认 latest 确认后成功升级**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[成功切换](../lifecycle.md#成功切换-200)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/update.rs)
- 说明：确认后走 switch 提交路径，事务 `operation` 为 `switch`，`.lkb` 与 systemd
  激活语义与 `lkit switch --version latest` 一致；fixture 用第二版本仓库 + pty
  确认断言 active 版本更新、服务保持运行。

## UP-03

**`--version` 固定目标确认后升级**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/update.rs)
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

## UP-08

**控制台 Update 面板分发时跳过 tty 交互**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[`lkit update`](../../../commands/update.md)、[交互控制台](../../../interaction/console.md)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：控制台把渠道选择与升级确认在 TUI 内完成（见 [UI-12](console.md#ui-12)），分发时
  标记 `--console-confirmed`：命令不再打开 `/dev/tty`，未显式 `--repository` 时按 switch
  规则解析来源，switch 流水线内部的交互确认同样视为已确认；目标解析、比较与执行不变。

## UP-09

**目标版本目录已存在且可信时更新复用已有目录**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[下载与发布目录](../../../repository.md#下载与发布目录)、[`switch_tests.rs` 复用用例](../../../../crates/lkit-cli/src/workflows/install/switch_tests.rs)、[Docker E2E S14](../../../docker-e2e.md#场景)
- 说明：`releases/<目标版本>` 残留（如上次升级失败自动回滚后）时，升级不再重复下载：
  已有目录通过可信校验（真实目录非符号链接、后端二进制与 `static/index.html` 齐全、
  `static.zip` 摘要与 manifest 一致、Identity 编码时二进制摘要一致）后直接复用并跳过下载；
  不可信或残缺目录仍以 `ReleaseExists` 阻断且不修改。复用规则与首次安装
  [INS-11](install.md#ins-11) 相同，switch 复用用例见 [SW-11](switch.md#sw-11)。
- 缺口：控制台 Update 面板分发入口的复用路径未单独做 E2E 断言（面板与命令共享同一 switch 流水线）。
