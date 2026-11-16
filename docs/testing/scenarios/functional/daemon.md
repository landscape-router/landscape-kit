# daemon 自动恢复场景

## DAE-01

**CLI 中断后 daemon 自动恢复遗留事务（激活中途消失 → 失败清理）**

- 测试层：Fixture E2E（`install_fixture_e2e::daemon`）
- 状态：`已覆盖`
- 证据：[事务与中断恢复](../../../deployment/transactions-and-recovery.md#daemon-自动恢复)
- 说明：构造 `activating` 阶段的 install 事务现场，启动 daemon 后断言事务标记
  `failed`、目标 release/current/初始化文件被清理；恢复后 daemon 继续运行，
  SIGTERM 时清理 pidfile 干净退出。

## DAE-02

**daemon 尊重安装锁：CLI 持有锁期间不触碰事务，释放后恢复**

- 测试层：Fixture E2E（`install_fixture_e2e::daemon`）
- 状态：`已覆盖`
- 说明：测试进程持锁期间断言事务保持原阶段，释放锁后 daemon 下一周期完成恢复；
  验证并发安全边界。

## DAE-03

**daemon 对网络接管待确认阶段不代替用户确认**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/daemon.rs)
- 说明：构造带 `network_takeover` 的 `awaiting_network_confirmation` 事务并启动
  daemon，多个周期后事务保持原阶段、安装现场不被触碰、daemon 存活；
  未到确认期限且未重启时等待；到期或主机重启回滚，提交中续做提交，回滚中重试。

## DAE-04

**daemon 恢复切换激活与提交阶段的崩溃**

- 测试层：CI Fixture E2E
- 状态：`已覆盖`
- 证据：[切换场景](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)，`daemon_switch_recovers_activation_and_commit_crashes_with_or_without_backup`
- 说明：覆盖有／无保护备份的激活中断、state 写入后和结果保存前崩溃；重启按原事务回滚或补齐成功回执，保持原操作身份，不重新执行切换。

## DAE-05

**daemon 启动时尽力拉起所有物理以太网卡，保证 flare L2 防失联通道可用**

- 测试层：Rust 单元测试（`network::ifup`，临时 sysfs 目录 + 假 `ip` 命令）
- 状态：`已覆盖（单元）`
- 证据：[flare 协议](../../../flare/protocol.md#部署形态)、`lkit-cli/src/network/ifup.rs`
- 缺口：真实网卡上的行为（对 DOWN 的物理网卡执行 `ip link set dev <name> up`）
  未纳入 fixture E2E
- 说明：网卡 DOWN 时 flare 服务端即使运行也无法收发帧，恢复通道失效。daemon
  启动、托管 flare 之前枚举 `/sys/class/net`，对处于 DOWN 状态的物理以太网卡
  执行 `ip link set dev <name> up`；跳过 loopback、无线与虚拟设备，无 MAC
  （`address` 缺失或为空）的网卡也跳过；已 UP 的网卡不发命令；任何失败只
  记录日志，不阻断 daemon 启动。

## DAE-06

**daemon 尽力准备 bpffs，失败保留 flare，Landscape 仅声明弱启动依赖**

- 测试层：Rust 单元测试、Docker flare e2e、QEMU Alpine/OpenRC（CI）
- 状态：`已覆盖`
- 证据：[bpffs 策略与失败继续](../../../../lkit-cli/src/deployment/bpffs.rs)、
  [检查状态](../../../../lkit-cli/src/check/kernel.rs)
  （`bpffs_ready_passes_and_all_failures_are_advisory`）、
  [fixture 隔离](../../../../lkit-cli/src/deployment/runtime.rs)
  （`fixture_runtime_never_prepares_host_bpffs`）、
  [systemd](../../../../lkit-cli/src/service/systemd.rs)、
  [OpenRC](../../../../lkit-cli/src/service/openrc.rs)、
  [SysVinit](../../../../lkit-cli/src/service/sysvinit.rs) 的服务定义渲染测试、
  [daemon flare e2e](../../../../scripts/flare/e2e-daemon.sh)、
  [Alpine 真实虚拟机 ALP-01 至 ALP-05](alpine.md)。
- 说明：策略单测注入状态与挂载动作，不调用真实挂载；Docker 使用只读 tmpfs 占用
  `/sys/fs/bpf`，验证错误日志之后 lflare 仍能建立会话。检查异常均为 warning，
  不增加安装阻断；服务定义不包含对 daemon 的硬依赖或 readiness 等待。



## DAE-07

**daemon 按确认期限与主机启动标识执行网络回滚**

- 测试层：Rust 单元、Fixture E2E（CI）
- 状态：`已覆盖`
- 证据：[恢复决策](../../../../lkit-cli/src/network/recovery.rs)、[daemon fixture](../../../../lkit-cli/tests/install_fixture_e2e/daemon.rs)、[恢复契约](../../../network/daemon-recovery.md)
- 说明：覆盖首次安装与 reinit 的截止时刻、同次启动继续等待、主机重启提前回滚、提交中续做和回滚重试决策。fixture 验证到期和模拟启动标识改变后清理未提交安装；本地不运行 fixture。

## DAE-08

**各 init 后端保存宿主服务启动状态并提供 Landscape 启动检查**

- 测试层：Rust 单元、服务定义渲染
- 状态：`部分覆盖`
- 证据：[宿主服务快照](../../../../lkit-cli/src/service/host_network.rs)、[恢复契约](../../../network/daemon-recovery.md)
- 缺口：三个后端分别在 CI 容器或虚拟机验收：待确认时重启，目标配置启动被阻断；
  daemon 恢复宿主网络并保留原启用设置；恢复动作失败后重试成功；已确认安装在重启后
  正常启动。OpenRC 验证多个 runlevel，SysVinit 验证 S/K 链接及顺序，systemd 验证
  enable/mask 状态。真实重启必须使用虚拟机，fixture 模拟 boot ID 不能替代该验收。
- 说明：临时目录验证 OpenRC 多 runlevel 与 SysVinit S/K 链接顺序、拒绝越界快照、注销服务清理启动链接和拒绝删除外部链接；注入服务动作验证恢复后的状态检查与失败重试。SysVinit 单测验证 `update-rc.d` 参数顺序、首次启用创建默认链接及真实 `rc?.d` 目录布局的启用检测。服务定义调用统一启动检查。

## DAE-09

**旧部署服务定义迁移与 reinit 准入**

- 测试层：隔离单元、CLI Fixture E2E（CI）
- 状态：`部分覆盖`
- 证据：[修复实现及测试](../../../../lkit-cli/src/workflows/repair_service.rs)、[CLI 迁移](../../../../lkit-cli/tests/install_fixture_e2e/repair_service.rs)
- 说明：覆盖三个后端运行／停止与启用／禁用组合、重复执行、定义漂移拒绝、加载失败、
  回滚失败重试及提交阶段前后的中断恢复；Fake 管理器禁止修改运行状态。
  CLI 迁移用例由 Fixture E2E 的 `repair-service` 分组执行，过滤器为 `repair_service::`。
- 缺口：三个后端真实迁移到 reinit 的完整链路与故障注入仍需 CI 验收。

## DAE-10

**reinit 回滚保留原服务运行状态**

- 测试层：隔离单元、CLI Fixture E2E
- 状态：`部分覆盖`
- 证据：[停止状态回归测试](../../../../lkit-cli/src/workflows/repair_service.rs)、[reinit fixture](../../../../lkit-cli/tests/install_fixture_e2e/reinit.rs)
- 说明：三个后端覆盖运行／停止 × 启用／禁用组合，原本停止时不启动、不做在线健康检查；
  原本运行时启动并检查健康；连续回滚两次保留原数据。
- 缺口：真实后端运行状态恢复的完整故障注入矩阵。

## DAE-11

**确认健康检查跨越截止时刻**

- 测试层：注入时间的单元测试
- 状态：`部分覆盖`
- 证据：[确认入口](../../../../lkit-cli/src/network/takeover.rs)、[期限测试](../../../../lkit-cli/src/network/recovery.rs)
- 说明：确认前后均校验时间与启动标识；覆盖截止前／截止时刻／截止后、启动标识改变或读取失败。
- 缺口：健康检查期间推进时间与确认／回滚锁竞争的完整 CLI 集成验证。

## DAE-12

**启动检查拒绝同次开机已超时事务和不可解析的恢复依据**

- 测试层：临时目录与注入时间的单元测试
- 状态：`部分覆盖`
- 证据：[启动检查](../../../../lkit-cli/src/network/recovery.rs)
- 说明：覆盖同次开机截止边界、跨开机与回滚阶段；启动检查只读，不等待安装锁。
- 缺口：三个后端真实启动屏障及损坏恢复凭据的完整验证。

## DAE-13

**daemon 恢复能力及持续退避重启**

- 测试层：隔离单元、CLI Fixture E2E、systemd-nspawn、Alpine QEMU（CI）
- 状态：`部分覆盖`
- 证据：[能力标识](../../../../lkit-cli/src/daemon.rs)、[退避策略](../../../../lkit-cli/src/daemon_supervisor.rs)、[进程 fixture](../../../../lkit-cli/tests/install_fixture_e2e/daemon.rs)、[systemd smoke](../../../../scripts/test-nspawn-systemd.sh)、[OpenRC 虚拟机](../../../../scripts/qemu/alpine/guest-tests.sh)
- 说明：单元覆盖能力缺失／PID 不符、1s→3s→5s→10s 封顶、无限重试及 5 分钟稳定重置；
  fixture 覆盖实际子进程崩溃、重复封顶拉起、主动停止与退避等待中停止；
  `daemon_handles_stop_after_pid_publication_before_startup_finishes` 在 PID 已发布、
  启动准备尚未结束时发送 SIGTERM，验证干净退出与 PID／能力标记清理。
  systemd／OpenRC 场景检查监管进程保持不变而 daemon 子进程重新启动。
- 缺口：SysVinit 真实虚拟机生命周期验收；本地不运行系统级测试，CI 执行结果单独判定。


## DAE-14

**持久化操作队列、串行领取与取消边界**

- 测试层：隔离单元测试
- 状态：`covered`
- 证据：[操作契约](../../../service/daemon.md)、[队列测试](../../../../lkit-cli/src/daemon_operations/tests.rs)
- 说明：按 ID 排队，终态不重放，排队可取消，执行中拒绝取消，持有安装锁时仍能查询。

## DAE-15

**操作记录与事务之间的崩溃窗口**

- 测试层：隔离单元测试
- 状态：`covered`
- 证据：[操作状态恢复](../../../../lkit-cli/src/daemon_operations.rs)、[崩溃窗口测试](../../../../lkit-cli/src/daemon_operations/tests.rs)
- 说明：关联先于事务创建；提交后未保存结果恢复为成功；回滚恢复为失败；无事务证据标记中断；损坏或身份不符保持恢复中。不以重新执行工作流猜测结果。

## DAE-16

**daemon 直接修复服务，前端断开与执行中强杀**

- 测试层：CI fixture e2e
- 状态：`covered`
- 证据：[真实进程场景](../../../../lkit-cli/tests/install_fixture_e2e/repair_service.rs)、[CI 分片](../../../../.github/workflows/test-fixture-e2e.yml)
- 说明：在安装锁下排队并关闭 CLI；释放锁后由 daemon 完成操作。fake systemctl 的父 PID 必须为 daemon PID；暂停 refresh 后强杀 daemon，重启完成回滚。终态重启后保持不变。

## DAE-17

**生命周期操作原生执行，前端只提交与观察**

- 测试层：隔离单元、CI Fixture E2E、systemd-nspawn
- 状态：`已覆盖`
- 证据：[CLI 参数约束](../../../../lkit-cli/src/cli.rs)、[队列测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[终端隔离](../../../../lkit-cli/src/interaction/interactive.rs)、[业务 fixture](../../../../lkit-cli/tests/install_fixture_e2e/)、[真实 systemd 场景](../systemd-smoke.md#sys-03)
- 说明：所有生命周期请求使用结构化参数，拒绝内部 worker 参数、迁移 resume 和网络计划文件传输参数。daemon 禁止读取终端；fixture 覆盖安装、重初始化、迁移、切换、修复、恢复、卸载及网络确认／回滚的持久化操作。真实 systemd 场景验证前端退出、daemon 崩溃恢复和运行中可执行路径被删除后继续执行。
