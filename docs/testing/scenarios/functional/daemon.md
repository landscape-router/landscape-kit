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

**daemon 周期恢复覆盖 systemd 失败切换（`.lkb` 配置级回滚）**

- 测试层：Rust workflow（`recover_switch`）、Fixture E2E 待补充
- 状态：`部分覆盖`
- 说明：`recover_interrupted` 的 switch 回滚语义已有 Rust 覆盖（仅 preparing 分支）；
  daemon 作为其触发者与 CLI 复用同一代码路径，daemon 侧的完整失败切换现场
  （activating 阶段 + `.lkb` 配置级回滚）待补充。

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

## 委托端到端链路（worker）覆盖现状

`daemon_worker` 的委托请求链路（CLI 写请求 → daemon 认领 → 子进程执行 → 结果回收）
单测只有 5 个薄用例（委托清单、worker flag 注入、凭据文件权限、完成消息）；fixture
E2E 全部以 `--test-runtime` 内联执行，从不走真实委托。真实形态的端到端验证在
[systemd-nspawn 兼容性 smoke](../systemd-smoke.md#sys-03)（`scripts/test-nspawn-systemd.sh`，
仅 CI/手动运行）：编译好的 lkit 进入真实 systemd 容器，`self install` 由 systemd
托管 daemon，CLI 以真实委托执行卸载——覆盖提交与结果回收（S-1）、前端断开后
daemon 独立完成（S-2）、cancel 文件驱动的取消 + daemon 恢复（S-3，uninstall
无下载阶段，前端 Ctrl+C 按"仅 Downloading 可取消"契约被忽略）、daemon 未运行
拒绝（S-4）、`LKIT_LANG` 转发（S-5）。

`delegate()` 请求文件生命周期与 executor 的 SIGTERM→SIGKILL 兜底（`CANCEL_GRACE_POLLS`
超时后强杀）仍无直接测试。

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

- 测试层：CLI Fixture E2E（待补充）
- 状态：`待实现`
- 证据：[reinit](../../../commands/reinit.md)、[恢复契约](../../../network/daemon-recovery.md)
- 缺口：实现 `repair service`；覆盖三个后端迁移成功后通过 reinit 准入、重复执行无变化、
  自定义定义或外部注册拒绝覆盖、替换后加载失败回滚、提交前中断恢复、恢复失败保留
  非终态。断言原运行状态、启用状态、权限与数据不变。单元测试注入服务管理器；
  真实 reload 和命令到命令的迁移链路在 CI 执行。

## DAE-10

**reinit 回滚保留原服务运行状态**

- 测试层：隔离单元、CLI Fixture E2E（待补充）
- 状态：`待修复`
- 证据：[reinit 回滚实现](../../../../lkit-cli/src/workflows/reinit.rs)
- 缺口：按运行／停止 × 启用／禁用验证恢复；原本停止且允许无保护备份时不调用
  start 或在线健康检查；原本运行时必须恢复并通过健康检查。覆盖失败重试、重复回滚、
  状态快照缺失时阻断而非猜测，以及各后端启用设置保持不变。当前实现无条件启动原服务。

## DAE-11

**确认健康检查跨越截止时刻**

- 测试层：注入时钟的单元、CLI Fixture E2E（待补充）
- 状态：`待修复`
- 证据：[确认实现](../../../../lkit-cli/src/network/takeover.rs)、[恢复契约](../../../network/daemon-recovery.md)
- 缺口：注入时钟覆盖检查完成时早于／等于／晚于截止时刻、检查期间 boot ID 改变或
  读取失败；过期不得写入 `finalizing` 或安装状态。覆盖确认与回滚竞争，确认持锁期间
  daemon 等待，释放后重新判断；`finalizing` 已持久化后即使超时或重启仍继续提交。
  当前实现仅在健康检查前校验截止时刻。

## DAE-12

**启动检查拒绝同次开机已超时事务和不可解析的恢复依据**

- 测试层：临时目录与注入时间的单元测试、CI 后端启动测试
- 状态：`待补充`
- 证据：[启动契约](../../../network/daemon-recovery.md#启动与生命周期)
- 缺口：截止前／截止时刻／截止后、跨开机、损坏事务和未知版本；持锁启动服务不死锁；
  旧配置恢复凭据不能放行其他事务或目标配置。当前启动检查没有同次开机的到期判定。

## DAE-13

**daemon 不可用与恢复能力不匹配**

- 测试层：注入能力探测的单元测试、CI 后端进程生命周期测试
- 状态：`待补充`
- 证据：[恢复责任与可用性](../../../network/daemon-recovery.md#启动与生命周期)
- 缺口：PID 存活但能力缺失时拒绝接管和服务迁移；新 CLI 与旧 daemon 混用；
  daemon 消失后重启立即处理过期事务；各后端崩溃监管行为；未完成事务拒绝不兼容降级。
  已提交安装在 daemon 重启期间保持运行，移除 daemon 后保留 CLI 时仍可执行启动检查。
