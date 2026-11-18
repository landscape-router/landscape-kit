# 版本更新场景

`lkit update` 是 [`lkit switch`](../../../commands/switch.md) 的交互式薄封装：选择读取渠道、
在前端解析目标版本、要求用户确认后提交结构化 switch 操作，由 daemon 直接执行。渠道选择、确认与零副作用拒绝是
update 独有的行为；事务、备份、回滚与退出码语义全部继承 switch 域（见
[switch.md](switch.md) 与 [rollback.md](rollback.md)）。

## UP-01

**交互选择渠道后展示当前与目标版本**

- 测试层：CLI/伪终端
- 状态：`部分覆盖`
- 证据：[`lkit update`](../../../commands/update.md)、[CLI fixture E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 缺口：默认镜像和 GitHub 来源在交互确认后的入口组合矩阵；HTTP 当前来源在确认后变更配置已有 fixture 验证。
- 说明：未显式 `--repository` 时通过 `/dev/tty` 选择渠道；预置 `config.toml` 时
  首个"当前来源"选项支持直接回车（update 不修改该文件），文件不存在时选项从官方
  GitHub 开始（默认选中）；列表同时显示 GitHub、Mirror 和自定义 HTTP。解析
  `latest` 后展示 `当前 <X> → 目标 <Y>`。

## UP-02

**默认 latest 确认后成功升级**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[成功切换](../lifecycle.md#成功切换-200)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/update.rs)
- 说明：前端解析并展示 latest 对应的精确版本，确认后固定目标和具体来源；daemon 不再
  查询 latest。检查活动版本、保护备份、服务状态和持久化操作结果。

## UP-03

**`--version` 固定目标确认后升级**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/update.rs)
- 说明：显式版本与默认 latest 在目标比较、确认和执行阶段遵循相同流程；降级行为继承
  [`SW-02`](switch.md#sw-02)，同版本行为由 `UP-04` 单独定义。

## UP-04

**已是最新时不变更并返回 `0`**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：目标与 `active_version` 相同时输出已是最新，返回 `0`，不创建事务、不下载资产，
  也不验证或持久化所选仓库来源；与 switch 进入同版本安装校验不同。

## UP-05

**拒绝确认时零副作用**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：确认在创建事务与备份之前；拒绝时退出码 `1`，不创建事务、不下载目标资产，
  `current`、state 与既有事务不变。

## UP-06

**非交互环境报错并提示使用 switch**

- 测试层：CLI
- 状态：`已覆盖`
- 证据：[CLI fixture E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：无 `/dev/tty` 或 `--non-interactive` 时返回普通失败（退出码 `1`），提示改用
  `lkit switch --version <VERSION>`；需要时再追加 `--repository <BASE_URL>`。

## UP-07

**升级失败自动回滚（继承 switch 语义）**

- 测试层：Docker E2E
- 状态：`部分覆盖`
- 证据：[失败切换与自动回滚](../rollback.md)、[S2/S3 扩展场景](../extended.md)、[update fixture](../../../../lkit-cli/tests/install_fixture_e2e/update.rs)
- 缺口：OpenRC／SysVinit 下 update 入口的真实进程回滚矩阵。
- 说明：执行与恢复复用 switch 操作，不定义独立回滚路径；switch 的恢复证据见 [SW-13](switch.md#sw-13) 和 [SW-14](switch.md#sw-14)。

## UP-08

**控制台 Update 面板分发时跳过 tty 交互**

- 测试层：Rust workflow/CLI
- 状态：`已覆盖`
- 证据：[`lkit update`](../../../commands/update.md)、[交互控制台](../../../interaction/console.md)、[CLI fixture E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：真实伪终端通过 [console fixture](../../../../lkit-cli/tests/install_fixture_e2e/console.rs) 验证确认后的单次提交与固定目标；确认上下文传递与失效由 [控制台单元测试](../../../../lkit-cli/src/console/tests/update.rs) 覆盖。
- 说明：面板完成解析与确认后提交该确认上下文，不传未解析的 latest 或默认来源。
  `--console-confirmed` 不替代完整上下文，不绕过服务变化和无备份切换授权，也不触发
  daemon 终端交互；改动表单后必须重新解析和确认。

## UP-09

**目标版本目录已存在且可信时更新复用已有目录**

- 测试层：Rust workflow、Docker E2E
- 状态：`部分覆盖`
- 证据：[下载与发布目录](../../../repository.md#下载与发布目录)、[`switch_tests.rs` 复用用例](../../../../lkit-cli/src/workflows/install/switch_tests.rs)、[Docker E2E S14](../../docker-lifecycle.md#场景)
- 说明：`releases/<目标版本>` 残留（如上次升级失败自动回滚后）时，升级不再重复下载：
  已有目录通过可信校验（真实目录非符号链接、后端二进制与 `static/index.html` 齐全、
  `static.zip` 摘要与 manifest 一致、Identity 编码时二进制摘要一致）后复制到事务临时目录复用并跳过版本资产下载；
  不可信或残缺目录仍以 `ReleaseExists` 阻断且不修改。复用规则与首次安装
  [INS-11](install.md#ins-11) 相同，switch 复用用例见 [SW-11](switch.md#sw-11)。
- 缺口：控制台 Update 面板分发入口的复用路径未单独做 E2E 断言（面板与命令共享同一 switch 流水线）。

## UP-10

**确认期间或排队期间安装根路径、版本或状态摘要改变时拒绝执行**

- 测试层：隔离请求单元测试、CI fixture E2E
- 状态：`部分覆盖`
- 证据：[`lkit update`](../../../commands/update.md)、[SW-12](switch.md#sw-12)
- 说明：同根同版本的 service 状态字段变化由 [update fixture](../../../../lkit-cli/tests/install_fixture_e2e/update.rs) 覆盖。检查不识别相同路径与状态内容下的目录替换。
- 说明：请求绑定展示时的安装根、活动版本和完整状态摘要。这些值变化时拒绝，要求重新
  确认，不以新现场重建请求，不下载目标资产或创建切换事务。SW-12 提供 daemon 领取
  时校验现场的证据，update fixture 验证确认时状态摘要不会在提交时被替换。

## UP-11

**提交后前端断开、重连、取消与崩溃恢复**

- 测试层：隔离操作单元测试、CI fixture E2E、控制台端到端
- 状态：`部分覆盖`
- 证据：[操作契约](../../../service/daemon.md)、[SW-12](switch.md#sw-12)、[SW-13](switch.md#sw-13)、[SW-14](switch.md#sw-14)
- 缺口：重新进入控制台观察既有操作的交互入口。
- 说明：[console fixture](../../../../lkit-cli/tests/install_fixture_e2e/console.rs) 验证 Ctrl+C 断开、Esc/Enter 显式取消、激活阶段拒绝取消与终端恢复；断开后通过 operation status 查询同一个操作。CLI SIGINT、daemon 激活后崩溃与提交后补写结果由 [update fixture](../../../../lkit-cli/tests/install_fixture_e2e/update.rs) 覆盖。
- 说明：Ctrl+C 和前端退出只停止观察，不重新提交或隐式取消。显式取消仅在排队或下载
  阶段接受，清理完成后返回 `130`。进入 preparing 后拒绝取消；已提交事务只补写
  结果，恢复失败阻止后续修改。daemon 不可用或能力不匹配时不回退到 worker。
