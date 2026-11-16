# 重新初始化场景

## REI-01

**reinit 拒绝未安装、服务后端不受支持 或未接管的安装**

- 测试层：CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[reinit 命令规格](../../../commands/reinit.md)、[管理入口](../../../../lkit-cli/src/commands/reinit.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：无有效状态返回参数错误（退出码 `2`，不写任何文件）；宿主网络服务未接管
  返回参数错误（退出码 `2`，不创建 reinit 事务、状态不动）。
- 说明：systemd、OpenRC 和 SysVinit 均使用 daemon 恢复契约。

## REI-02

**凭据与网络计划先于任何修改收集,拒绝确认时不落盘**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络重配置](../../../network/reinit.md)、[网络发现](../../../../lkit-cli/src/network/discovery.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：凭据与网络计划在事务创建前收集；交互确认拒绝（退出码 `1`）或非交互缺少
  `--yes`（参数错误退出码 `2`）时不创建事务、不创建 `.lkb`、不停止服务、
  不改写数据。

## REI-03

**停止服务前创建 `reinit 前自动备份` 保护 `.lkb`**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[`.lkb` 备份与回滚](../../../backup/lkb-and-rollback.md)
- 说明：接管安装确认后执行 reinit,断言 `backups/` 数量增加且新 `landscape_init.toml`
  携带新凭据；导出 API 失败阻断与 `--allow-no-backup` 路径尚未覆盖。

## REI-04

**旧 data 移入事务目录,新空 data 按新 init 配置重建**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[reinit 工作流](../../../../lkit-cli/src/workflows/reinit.rs)
- 说明：E2E 断言重建后 `landscape_init.toml` 的 `version` 等于当前活动版本、包含新
  凭据，确认提交后存在初始化锁；release 与静态资产保持不变的断言在回滚 E2E 中体现。

## REI-05

**reinit 不检查 br_lan,桥接现场由 Landscape 处理**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`部分覆盖`
- 证据：[网络重配置](../../../network/reinit.md)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：install 与 reinit 均不检查 `br_lan` 是否存在、不执行成员操作；新选 LAN 接口
  的地址 flush 保留。E2E 覆盖 reinit 主路径与回滚，真实桥接现场验证待 QEMU 环境。

## REI-06

**健康检查通过后一律进入确认窗口,confirm 复核后提交**

- 测试层：CLI fixture E2E、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[控制台测试](../../../../lkit-cli/src/console/tests/reinit.rs)
- 说明：E2E 断言 reinit 事务进入 `awaiting_network_confirmation` 且 pending state 已
  写入；`lkit network confirm` 复核后提交为 `committed`，移除恢复 unit。

## REI-07

**未确认回滚恢复旧 data 与旧配置**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[事务与中断恢复](../../../deployment/transactions-and-recovery.md)
- 说明：E2E 断言手工 `lkit network rollback` 后事务 `rolled_back`、旧 `landscape_init.toml`
  逐字节恢复、原数据库文件存在且恢复 unit 已移除；daemon 期限到达与主机重启恢复 入口与
  首次接管共用同一回滚实现。

## REI-08

**激活或健康检查失败自动回滚,退出码 `5`;回滚失败 `6`**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[reinit 工作流](../../../../lkit-cli/src/workflows/reinit.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：健康检查注入失败（base_url 指向不可达端口）→ 激活失败自动回滚；注入
  同时使回滚后的健康检查失败，验证 `RollbackFailed` 分支（退出码 `6`、事务标记
  `rolling_back` 并重试）：数据在健康检查前已恢复（`landscape_init.toml` 复原）、事务不进入
  确认窗口。注入只作用于激活阶段时走 `RolledBack` 分支（退出码 `5`）。
- 缺口：激活/健康检查失败自动回滚（退出码 `5`）与回滚失败（退出码 `6`）无 fixture
  注入场景；switch/restore 的同类回滚测试不覆盖 reinit。

## REI-09

**中断事务按阶段恢复,待确认阶段阻断其他命令**

- 测试层：CLI fixture E2E、Rust 事务测试
- 状态：`已覆盖`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md)、[事务测试](../../../../lkit-cli/src/deployment/transaction/mod.rs)
- 说明：单元测试覆盖 `preparing` 标记 failed、`awaiting_network_confirmation` 恢复时
  阻断并提示 `lkit network confirm`/`rollback`；`activating`/`verifying` 的旧 data
  回滚与手工 rollback E2E 共用同一实现。

## REI-10

**控制台 Reinit 面板执行向导、确认与待确认提示屏**

- 测试层：Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台测试](../../../../lkit-cli/src/console/tests/reinit.rs)
- 说明：覆盖面板可用性门禁、向导完成进入凭据步骤、凭据编辑与确认层、结构化
  `Reinit` 请求构建（`--console-confirmed`、`--yes`、确认时安装身份、密码与网络计划经结构化操作传递）、
  Esc 取消。

## REI-11

**原生 reinit 的凭据交接、激活中断与等待网络确认**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[操作测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[reinit fixture](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)、[reinit 契约](../../../commands/reinit.md)
- 说明：密码不进入操作 JSON，独立文件权限为 `0600`，读取后、排队取消和认领中断时
  删除；操作等待确认时阻挡普通请求但允许绑定事务的确认。CI 在替换数据后和进入确认
  窗口后分别断开观察者并强杀 daemon，验证原数据回滚、确认期限不变和原操作结果更新。
- 缺口：OpenRC／SysVinit 原生链路故障注入矩阵。

## REI-12

**全屏待确认页跟随外部确认与自动回滚**

- 测试层：隔离单元、PTY Fixture E2E（CI）
- 状态：`已覆盖`
- 证据：[观察页单元测试](../../../../lkit-cli/src/interaction/presentation/operation.rs)、[真实 TUI reinit](../../../../lkit-cli/tests/install_fixture_e2e/console.rs)（`console_reinit_refreshes_external_confirmation_and_automatic_rollback`）
- 说明：TUI 完成网络向导、凭据输入和破坏性确认后，观察同一原生操作。确认层打开期间，由另一会话确认或由 daemon 到期回滚；页面更新最终状态、关闭失效确认层，退出返回最新回执码，操作与事务 ID 不变。单元另覆盖恢复中、恢复失败和最终失败的切换，以及普通 revision 更新保留有效确认层。

## REI-13

**取消确认后重新读取安装身份**

- 测试层：隔离单元、PTY Fixture E2E（CI）
- 状态：`已覆盖`
- 证据：[控制台单元测试](../../../../lkit-cli/src/console/tests/reinit.rs)（`reinit_reopening_cancelled_confirmation_refreshes_installation_identity`）、[真实 TUI reinit](../../../../lkit-cli/tests/install_fixture_e2e/console.rs)
- 说明：确认层打开时保持原安装身份；Esc 取消后丢弃上下文。安装状态发生变化后重新打开确认层，读取新身份并正常提交；原生 TUI 场景验证该流程能继续进入网络确认窗口。
