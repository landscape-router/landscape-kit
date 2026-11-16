# 修复场景

工作流单元测试验证资产处理、备份和回滚；CLI／TUI 端到端测试通过结构化请求调用
同一 daemon 操作入口，核对操作 ID、事务关联、业务退出码和安装现场。测试中的
服务管理命令由 daemon 发起。每个业务场景同时描述执行链路与业务结果。

## REP-01

**`repair binary` 从可信仓库恢复被篡改后端并保持版本不变**

- 测试层：隔离工作流单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)、[工作流测试](../../../../lkit-cli/src/workflows/repair.rs)、[请求契约](../../../service/daemon.md#repair-请求与执行上下文)
- 缺口：交互控制台的端到端请求一致性与安装状态变化的完整竞争矩阵。
- 说明：请求按类型校验参数，区分仓库四种选择；不支持的操作类型或协议在入队前拒绝。
  领取后重新核对安装根和版本，现场变化时失败。成功时验证后端身份、版本不变、
  事务已提交及操作成功；重复查询结果不产生第二次修复。

## REP-02

**`repair static` 恢复静态页面并保持资产与安装状态一致**

- 测试层：隔离工作流单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[工作流与快照测试](../../../../lkit-cli/src/workflows/repair.rs)、[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)、[静态修复阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)
- 缺口：归档写入中的磁盘故障及断电持久性矩阵。
- 说明：目录移出、目录替换、state 写入三个窗口分别覆盖原归档存在与缺失时的真实 daemon 强杀恢复。
  成功后核对页面、归档和 state；服务 PID 不变、不创建 `.lkb`、不修改 data。
  覆盖原归档不存在的情况；在备份、归档替换、目录切换及 state 写入时注入故障，
  提交前恢复一致的旧现场。自定义前端仅恢复实际改动的对象。

## REP-03

**`repair static` 按选定来源校验并恢复页面**

- 测试层：隔离工作流单元测试、本地 HTTP fixture、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[`lkit repair`](../../../commands/repair.md)、`repairs_static_from_a_repository_with_different_assets`（lkit-cli/src/workflows/repair.rs）、[FE-06](frontend.md#fe-06)
- 缺口：执行期间来源配置变化的端到端验证。
- 说明：state 记录的 static 身份与仓库元数据不一致时，以下载来源的元数据校验包，
  恢复官方页面并更新 state 身份及 `static.zip`。一次操作中来源与校验身份保持固定。
  自定义源和显式官方修复的行为由 FE-06 验证。

## REP-04

**binary repair 激活失败后使用 `.lkb` 和旧二进制回滚**

- 测试层：隔离工作流单元测试、fake manager、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)、`binary_repair_rolls_back_when_activation_fails`（lkit-cli/src/workflows/repair.rs）
- 缺口：OpenRC／SysVinit 真实进程场景的失败与恢复矩阵；三个后端的工作流使用内存 fake manager 覆盖。
- 说明：激活或健康检查失败时，使用 `.lkb`、旧二进制及现场快照恢复；核对二进制、
  state、服务状态和必要宿主文件，`current` 不变。事务为 `rolled_back`，操作报告
  修复失败，CLI 返回业务退出码 `5`。

## REP-05

**binary 或 static repair 恢复失败时保留现场并阻断后续修改**

- 测试层：隔离工作流单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：`binary_repair_rollback_failure_returns_rollback_failed`（lkit-cli/src/workflows/repair.rs）、[恢复边界](../../../service/daemon.md#repair-的恢复边界)、[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)
- 缺口：二进制回滚失败与磁盘空间耗尽的真实进程矩阵。
- 说明：静态目录目标被文件占用时，验证恢复失败、重启后继续阻断排队请求，以及解除占用后的自动恢复。
  回滚失败返回人工恢复结果和业务退出码 `6`，保留备份、快照及诊断现场。
  操作与事务分别报告结果和恢复依据；不能将恢复失败标为成功或取消，不能在现场
  尚未安全恢复时执行下一项修改。

## REP-06

**binary repair 使用受管后端完成保护备份、停启与健康检查**

- 测试层：隔离工作流单元测试、fake manager、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[Repair 阶段](../../../workflows/lifecycle.md#4-repair-阶段转换)、[请求执行上下文](../../../service/daemon.md#repair-请求与执行上下文)
- 缺口：OpenRC／SysVinit 真实进程资产修复，以及导出、备份失败的完整故障注入矩阵。
- 说明：无法管理实例、导出失败或保护备份失败时，在修改前终止；备份完整落盘并
  校验后才能停服务。由安装记录中的后端执行停启与健康检查，daemon 不读取终端，
  不等待人工停止进程。修复不改变活动版本。

## REP-07

**排队与下载阶段的取消竞争**

- 测试层：注入执行器的单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[取消契约](../../../service/daemon.md#进度与取消)、[排队取消测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)
- 缺口：磁盘空间耗尽及只读文件系统下的故障矩阵。
- 说明：static／binary 的事务目录被文件占用时，覆盖取消清理失败、重启阻断后续修改、修复目录后完成清理且不重下资源。
  覆盖取消先于领取、领取先于取消、取消先于窗口关闭、窗口关闭先于取消；
  接受下载取消后不得导出、备份、刷新 `static.zip` 或停服务。清理失败保持恢复中，
  重启继续清理，不重新下载；错误或中断不能冒充已取消。

## REP-08

**前端断开、进度重连与终态稳定性**

- 测试层：隔离单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[进度契约](../../../service/daemon.md#进度与取消)、[操作层测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)
- 缺口：交互终端 Ctrl+C、下载字节快照合并和控制台重连展示的完整端到端验证。
- 说明：查询不等待安装锁；重连不创建操作或事务；进度修订号不倒退，延迟进度不得
  覆盖终态；daemon 执行期间状态查询和恢复通道仍可响应。

## REP-09

**资产修复关联事务并在重启后避免重放**

- 测试层：注入事务状态的单元测试、CI fixture e2e
- 状态：`部分覆盖`
- 证据：[通用操作恢复测试](daemon.md#dae-15)、[资产操作测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[daemon repair fixture](../../../../lkit-cli/tests/install_fixture_e2e/repair.rs)、[恢复边界](../../../service/daemon.md#repair-的恢复边界)
- 缺口：操作记录原子写入本身的磁盘故障及断电持久性验证。
- 说明：static／binary 均在事务提交后、操作结果保存前强杀 daemon，验证重启只补写成功结果，下载、备份、事务和服务修改调用不增加。
  覆盖已领取未关联、关联先于事务、事务未完成、事务提交后结果未保存、事务
  损坏与归属不符。核对下载、备份、替换和服务调用次数；提交后只补写结果，不重放
  工作流。事务提交前的现场恢复分别由 REP-02、REP-04 和 REP-05 验证。
