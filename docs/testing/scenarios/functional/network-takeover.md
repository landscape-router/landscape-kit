# 网络接管场景

## NET-01

**双网口安装选择 LAN 时生成 br_lan route 与 DHCP**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络配置测试](../../../../lkit-cli/src/network/config.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：仅当用户至少选择一个 LAN 时创建 `br_lan`；空 LAN 使用 WAN-only 计划。

## NET-09

**多网口 CLI 允许 LAN 为空并按 WAN-only 计划安装**

- 测试层：Rust 单元、CLI 交互
- 状态：`已覆盖`
- 证据：[网络发现](../../../../lkit-cli/src/network/discovery.rs)、[网络配置](../../../../lkit-cli/src/network/config.rs)
- 说明：LAN 选择提示接受空输入；空集合转换为 `WanOnly`，不创建 `br_lan` 或 LAN DHCP。选择一个或多个 LAN 时仍使用 RoutedLan。

## NET-10

**WAN IPv4 配置与所选 LAN 地址清理遵循网络计划**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络配置](../../../../lkit-cli/src/network/config.rs)、[网络发现](../../../../lkit-cli/src/network/discovery.rs)、[网络接管](../../../../lkit-cli/src/network/takeover.rs)
- 说明：CLI 发现完整地址/网关时取所选 WAN 的第一个 IPv4 作为静态配置，否则使用 DHCP；
  停止宿主网络服务后只清理所选 LAN 的 IPv4/IPv6 地址。

## NET-02

**单网口保留 SSH IPv4/网关并为 TCP 22、6443 创建 Local 静态映射**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[网络配置测试](../../../../lkit-cli/src/network/config.rs)

## NET-03

**接口始终由用户选择，选择结果与 MAC 写入事务供确认复核**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[接口发现](../../../../lkit-cli/src/network/discovery.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)

## NET-04

**daemon 恢复元数据在停止宿主网络服务前持久化**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)

## NET-05

**NetworkManager 或 Debian ifupdown 的 `networking.service`、firewalld、systemd-resolved 在 systemd 下先统一 mask 再停止，保留启动链接且不卸载软件包**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)
- 说明：覆盖 NetworkManager 缺失且 `networking.service` 处于 active/enabled 状态的接管与回滚。

## NET-06

**任意可达会话均可确认并提交安装，TUI 以待确认阻塞屏提示**

- 测试层：CLI fixture E2E、QEMU/KVM、Ratatui TestBackend
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[QEMU 网络接管](../../qemu-network-takeover.md)、[控制台测试](../../../../lkit-cli/src/console/)
- 说明：`lkit network confirm` 不校验 SSH 会话来源，在任意可达会话（含本地控制台）均可
  运行；双网口在确认前保留 WAN 地址，确认检查通过后按 Static 或 DHCP 计划验证；网络
  计划校验失败不提交。进入
  TUI 时若存在待确认网络接管，直接显示阻塞屏（“稍后”退出、“确认执行”内联运行
  `lkit network confirm`），Install 菜单不可进入。
- 缺口：确认时 `verify_interfaces`/`verify_live` 校验失败不提交的路径无直接断言。

## NET-07

**未确认回滚清理安装并精确恢复宿主网络服务状态**

- 测试层：CLI fixture E2E、QEMU/KVM
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[QEMU 网络接管](../../qemu-network-takeover.md)
- 说明：覆盖手工 rollback、daemon 超时回滚 和确认前主机重启回滚；三条入口
  都必须恢复宿主网络、删除未提交首次安装的整个 `data/`，并允许随后带新凭据重新执行
  `lkit install`。
- 缺口：fixture 直接覆盖自动回滚入口和重装，QEMU 覆盖 boot rollback；真实确认期限到达和
  真实主机启动恢复仍需按 QEMU 场景验证；原生手工回滚见 NET-13。

## NET-11

**网络接管回滚清理失败时保留现场并保持 rolling_back，不伪造 rolled_back**

- 测试层：CLI fixture E2E、Rust 事务测试
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[事务与中断恢复](../../../deployment/transactions-and-recovery.md#未提交网络接管安装的回滚清理)
- 说明：fixture 通过异常 `current` 链接注入清理失败，断言退出码 `6`、事务为 `rolling_back` 且
  残留 data 未被删除。

## NET-12

**目标 release ≥ 0.25.1 时初始化配置由 `landscape config` 子命令生成**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[config 子命令适配层](../../../../lkit-cli/src/network/config_cli.rs)、[fixture 生成器](../../../../crates/lkit-test-fixture/src/config_cli.rs)、[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)、[网络重配置·新配置生成](../../../network/reinit.md)
- 说明：lkit 调用目标 release 目录下 `landscape-webserver config --stdout`（内嵌目标
  二进制版本，install 与 reinit 共用）；版本门槛为 `>= 0.25.1`（预发布版本不算达标），
  更早版本与无网络计划的安装维持 lkit 手拼路径。fixture 的 `landscape-webserver` 实现了
  同名子命令（版本从 release 目录名推导），e2e 因此走真实子命令路径并断言生成文件结构。
  两条路径服务集一致：WAN 上 firewall、不启用 nat，DHCP 租期 43200 秒。

## NET-08

**SELinux 与不受支持的活动网络管理器在任何网络变更前阻断**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[接口发现](../../../../lkit-cli/src/network/discovery.rs)
- 说明：`networking.service` 是受支持的 Debian ifupdown 宿主服务，不属于本场景的未知
  管理器。已有 `br_lan` 不阻断安装：install 与 reinit 都不检查桥接是否存在，桥接的
  创建、成员同步与清理由 Landscape 按新配置处理。

## NET-13

**原生确认与手工回滚绑定事务，前端断开和 daemon 崩溃不丢失结果**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[操作队列测试](../../../../lkit-cli/src/daemon_operations/tests.rs)、[网络 fixture](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)、[网络命令](../../../commands/network.md)
- 说明：验证网络操作可处理阻挡普通队列的事务、拒绝事务 ID 不匹配、只允许排队取消。
  确认与回滚分别在接受动作后及事务终结但结果尚未写入时强杀 daemon，重启后检查
  原操作 ID 的持久化成功结果；回滚恢复宿主服务并清理未提交的数据。
- 缺口：原生操作链路的 OpenRC／SysVinit 故障注入矩阵尚未单独覆盖。

## NET-14

**networkd 服务组自动接管、确认与恢复**

- 测试层：Rust 单元、CLI fixture E2E、真实 systemd
- 状态：`已覆盖`
- 证据：[服务组单元测试](../../../../lkit-cli/src/service/host_systemd.rs)、[网络 fixture](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)、[真实 systemd 场景](../systemd-smoke.md#sys-07)、[接管规范](../../../network/takeover.md)
- 说明：检查整组先屏蔽后停止、socket 与服务的不同启用状态、原始 mask、缺失单元及带转义名称的 wait-online 实例。回滚只移除指定 mask，保留指向该路径的相对启动链接；外部改动的文件或链接不被覆盖。确认保留屏蔽，回滚恢复原始运行状态；网络确认和回滚的 daemon 崩溃场景包含 networkd。启用链接与网络配置保持原样。

## NET-15

**回滚时 NetworkManager 恢复原 WAN 连接与 IPv4 可达性**

- 测试层：Rust 单元、QEMU/KVM（真实 NetworkManager 与 IPv6 RA）
- 状态：`已覆盖`
- 证据：[连接恢复单元测试](../../../../lkit-cli/src/network/network_manager.rs)、[QEMU 入口](../../../../scripts/test-qemu-network-takeover.sh)、[恢复故障注入](../../../../scripts/qemu/network/recovery-check.sh)
- 说明：NetworkManager 启动前等待 WAN 收到 RA；原 DHCP profile 暂时缺失时事务保持 rolling_back，待恢复 profile 后 daemon 自动激活原 UUID，恢复 IPv4、默认网关和 WAN SSH。单元测试验证接口身份变化、连接删除、IPv6-only、DHCP 尚未完成时不重复激活、旧快照兼容、采集期间连接变化、多连接中单个激活失败不阻塞其他连接恢复。

## NET-16

**所有 systemd 宿主网络服务先屏蔽再停止，保留原启动链接**

- 测试层：Rust 单元、CLI fixture E2E、QEMU/KVM
- 状态：`已覆盖`
- 证据：[宿主服务组](../../../../lkit-cli/src/service/host_systemd.rs)、[回滚 fixture](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)、[自定义启动链接](../../../../scripts/qemu/network/setup.sh)
- 说明：在停止第一个服务之前屏蔽整组，不调用 disable/enable；恢复只移除精确 mask，检查运行状态并还原原 mask。验证 NetworkManager 的绝对、相对自定义启动链接在真实回滚后完整保留；外部文件冲突不被覆盖。
