# 版本切换场景

`lkit switch` 只允许向更高的 stable 版本升级。较低版本必须在改变安装前拒绝；相同版本
沿用同版本安装校验，不创建版本切换事务。

## SW-01

**daemon 执行受管安装的版本升级**

- 测试层：CI fixture E2E、Docker E2E
- 状态：`部分覆盖`
- 证据：[daemon switch fixture](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)、[成功切换](../lifecycle.md#成功切换-200)
- 缺口：OpenRC／SysVinit 真实进程切换与恢复矩阵。
- 说明：结构化操作关联 schema v8 事务；daemon 直接完成备份、停启、健康检查及结果保存。

## SW-02

**显式切换到较低的历史 stable 版本时拒绝且不改变安装**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[版本切换工作流](../../../../lkit-cli/src/workflows/switch.rs)、[`lkit switch`](../../../commands/switch.md)
- 说明：使用 `0.22.2 → 0.21.1` 断言返回参数用法错误，`current`、state 和既有事务不变，
  且不下载目标二进制或静态资产。

## SW-03

**切换目标等于当前 active version 时不创建无意义事务**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)
- 说明：daemon 领取后解析目标，等于当前版本时验证安装并保存成功操作结果，不创建
  switch 事务、不重启服务。显式仓库覆盖需要下载并校验来源后端，不能将不同资产视为同版本成功。

## SW-04

**切换执行完整停止、激活与健康检查**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：[激活语义](../../../workflows/lifecycle.md#6-激活)

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
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)
- 说明：switch.rs：服务运行中 + `--allow-no-backup` → 警告"已忽略该标志"、
  事务 `no_backup: false`、照常创建 `.lkb`、切换成功、服务保持运行；见
  [`lkit switch`](../../../commands/switch.md#停止服务后的切换)。

## SW-11

**目标版本目录已存在且可信时切换复用已有目录**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[下载与发布目录](../../../repository.md#下载与发布目录)、[`switch_tests.rs` 复用用例](../../../../lkit-cli/src/workflows/install/switch_tests.rs)、[Docker E2E S14](../../docker-lifecycle.md#场景)
- 说明：`releases/<目标版本>` 残留（如上次切换失败自动回滚后）时，切换不再重复下载：
  已有目录通过可信校验（真实目录非符号链接、后端二进制与 `static/index.html` 齐全、
  `static.zip` 摘要与 manifest 一致、Identity 编码时二进制摘要一致）后直接复用并跳过下载；
  不可信或残缺目录仍以 `ReleaseExists` 阻断且不修改。复用规则与首次安装
  [INS-11](install.md#ins-11) 相同，update 入口场景见 [UP-09](update.md#up-09)。

## SW-12

**排队请求绑定现场，下载取消不修改当前安装**

- 测试层：隔离操作单元测试、CI fixture E2E
- 状态：`部分覆盖`
- 证据：[操作测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[daemon switch fixture](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)
- 缺口：真实磁盘清理故障及交互终端 Ctrl+C 展示验证。
- 说明：排队期间 state 改变时拒绝领取执行，不下载、不创建事务。前端断开不取消已接受
  的下载；显式取消清理事务临时目录，保留既有 release、current 和服务状态。取消与
  preparing 竞争在持久化操作锁下决定，进入激活后拒绝取消。

## SW-13

**激活与状态写入后崩溃时使用旧快照恢复**

- 测试层：CI fixture E2E
- 状态：`部分覆盖`
- 证据：[daemon switch fixture](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)、[事务规则](../../../deployment/transactions-and-recovery.md#daemon-switch-事务)
- 缺口：目标目录发布中途和服务 stop 中途的强杀矩阵。
- 说明：有 `.lkb` 和显式无备份两种路径均在 current 切换后、state 写入后强杀 daemon；
  重启恢复旧状态、current 和服务 active 状态。无备份路径保持旧服务停止，不从新 state
  推断旧状态，也不承诺恢复 data。

## SW-14

**提交后补写结果及回滚失败阻断后续修改**

- 测试层：隔离操作单元测试、CI fixture E2E
- 状态：`部分覆盖`
- 证据：[操作测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[daemon switch fixture](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)
- 缺口：结果写入本身的磁盘故障与真实断电持久性验证。
- 说明：事务提交后结果尚未保存时强杀，只补写成功结果，不重新下载或停启服务。
  `.lkb` 损坏导致回滚失败时保持 recovering 和 rolling_back，后续修改请求保持排队；
  修复备份后恢复旧版本并报告失败、退出码 `5`。
