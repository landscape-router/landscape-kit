# 重新初始化场景

## REI-01

**reinit 拒绝未安装、非 systemd 或未接管的安装**

- 测试层：CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[reinit 命令规格](../../../commands/reinit.md)、[管理入口](../../../../crates/lkit-cli/src/commands/reinit.rs)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：无有效状态返回参数错误（退出码 `2`，不写任何文件）；宿主网络服务未接管
  返回参数错误（退出码 `2`，不创建 reinit 事务、状态不动）。
- 缺口：非 systemd manager（退出码 `2`）拒绝分支无命令层测试。

## REI-02

**凭据与网络计划先于任何修改收集,拒绝确认时不落盘**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络重配置](../../../network/reinit.md)、[网络发现](../../../../crates/lkit-cli/src/network/discovery.rs)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：凭据与网络计划在事务创建前收集；交互确认拒绝（退出码 `1`）或非交互缺少
  `--yes`（参数错误退出码 `2`）时不创建事务、不创建 `.lkb`、不停止服务、
  不改写数据。

## REI-03

**停止服务前创建 `reinit 前自动备份` 保护 `.lkb`**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[`.lkb` 备份与回滚](../../../backup/lkb-and-rollback.md)
- 说明：接管安装确认后执行 reinit,断言 `backups/` 数量增加且新 `landscape_init.toml`
  携带新凭据；导出 API 失败阻断与 `--allow-no-backup` 路径尚未覆盖。

## REI-04

**旧 data 移入事务目录,新空 data 按新 init 配置重建**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[reinit 工作流](../../../../crates/lkit-cli/src/workflows/reinit.rs)
- 说明：E2E 断言重建后 `landscape_init.toml` 的 `version` 等于当前活动版本、包含新
  凭据，确认提交后存在初始化锁；release 与静态资产保持不变的断言在回滚 E2E 中体现。

## REI-05

**reinit 不检查 br_lan,桥接现场由 Landscape 处理**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`部分覆盖`
- 证据：[网络重配置](../../../network/reinit.md)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：install 与 reinit 均不检查 `br_lan` 是否存在、不执行成员操作；新选 LAN 接口
  的地址 flush 保留。E2E 覆盖 reinit 主路径与回滚，真实桥接现场验证待 QEMU 环境。

## REI-06

**健康检查通过后一律进入确认窗口,confirm 复核后提交**

- 测试层：CLI fixture E2E、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[控制台测试](../../../../crates/lkit-cli/src/console/tests/reinit.rs)
- 说明：E2E 断言 reinit 事务进入 `awaiting_network_confirmation` 且 pending state 已
  写入；`lkit network confirm` 复核后提交为 `committed`，移除恢复 unit。

## REI-07

**未确认回滚恢复旧 data 与旧配置**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[事务与中断恢复](../../../deployment/transactions-and-recovery.md)
- 说明：E2E 断言手工 `lkit network rollback` 后事务 `rolled_back`、旧 `landscape_init.toml`
  逐字节恢复、原数据库文件存在且恢复 unit 已移除；timer 到期与 boot rollback 入口与
  首次接管共用同一回滚实现。

## REI-08

**激活或健康检查失败自动回滚,退出码 `5`;回滚失败 `6`**

- 测试层：CLI fixture E2E
- 状态：`待补充`
- 证据：[reinit 工作流](../../../../crates/lkit-cli/src/workflows/reinit.rs)
- 说明：通过 fixture 注入新配置启动失败场景尚未覆盖（`REI-08`）；回滚优先使用事务
  目录旧 data 现场，回滚失败保留现场、事务标记 `failed`，返回 `6`。
- 缺口：激活/健康检查失败自动回滚（退出码 `5`）与回滚失败（退出码 `6`）无 fixture
  注入场景；switch/restore 的同类回滚测试不覆盖 reinit。

## REI-09

**中断事务按阶段恢复,待确认阶段阻断其他命令**

- 测试层：CLI fixture E2E、Rust 事务测试
- 状态：`已覆盖`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md)、[事务测试](../../../../crates/lkit-cli/src/deployment/transaction/mod.rs)
- 说明：单元测试覆盖 `preparing` 标记 failed、`awaiting_network_confirmation` 恢复时
  阻断并提示 `lkit network confirm`/`rollback`；`activating`/`verifying` 的旧 data
  回滚与手工 rollback E2E 共用同一实现。

## REI-10

**控制台 Reinit 面板执行向导、确认与待确认提示屏**

- 测试层：Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台测试](../../../../crates/lkit-cli/src/console/tests/reinit.rs)
- 说明：覆盖面板可用性门禁、向导完成进入凭据步骤、凭据编辑与确认层、结构化
  `Reinit` 请求构建（`--console-confirmed`、`--yes`、密码与网络计划经委托通道传递）、
  Esc 取消。
