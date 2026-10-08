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
  摘除宿主网络管理后只清理所选 LAN 的 IPv4/IPv6 地址。

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

**恢复 timer 与 boot rollback unit 在停止宿主网络服务前持久化并启动**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)

## NET-05

**systemd-resolved 被停止、disable、mask，但软件包不卸载；`networking.service`、NetworkManager 与 firewalld 保持运行**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)
- 说明：整体 stop/disable/mask 只作用于 systemd-resolved（DNS 是主机全局语义，没有按
  接口摘除的文件边界）；NetworkManager 与 firewalld 走 NET-14 的 drop-in/zone 细粒度
  摘除，systemd-networkd 走 NET-15 的 `.network` 移出，ifupdown 宿主的
  `networking.service` 走 NET-13。运行中的 NM/firewalld/networkd 配置目录缺失时
  preflight 拒绝接管。

## NET-13

**ifupdown 宿主细粒度摘除：选中接口改写 manual、退出 auto/allow-*，未选接口与 `networking.service` 不动**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络接管摘除实现](../../../../lkit-cli/src/network/takeover.rs)、
  [完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)
- 说明：接管把选中接口（WAN + 全部选中 LAN）的 stanza 改写为 `manual` 并删除选项与
  自动选择项，未选接口逐字节保留；原文件逐字备份到 lkit 地盘
  `backups/hostnet/ifupdown`；
  接管期间对 `networking.service` 零 systemctl 调用（保持 active/enabled/unmasked）；
  回滚按 manifest 逐字恢复、删除备份并在服务 active 时 restart。选中接口不由
  ifupdown 管理（无配置文件，如 NetworkManager 主机）时为 no-op。

## NET-14

**NetworkManager 与 firewalld 细粒度摘除：drop-in 与 zone 接口行，服务全程运行**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络接管摘除实现](../../../../lkit-cli/src/network/takeover.rs)、
  [完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)
- 说明：NM conf.d 存在时写入 `lkit-unmanage.conf` drop-in（NM 未运行也写，覆盖未来
  启动），firewalld zones 存在时删除引用选中接口的 `<interface/>` 整行；适配器可组合，
  备份落 `backups/hostnet/{nm,firewalld}`。文件改写后对运行中的 NM/firewalld reload
  （`nmcli general reload`、`firewall-cmd --reload`），工具缺失或 reload 失败中止安装；
  两个服务全程零 systemctl 调用。回滚/卸载删除 drop-in、逐字恢复 zone 并再次 reload。

## NET-15

**systemd-networkd 细粒度摘除：移出引用选中接口的 `.network` 文件，服务全程运行**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络接管摘除实现](../../../../lkit-cli/src/network/takeover.rs)、
  [完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/network.rs)、
  [networkd 适配器测试](../../../../crates/lkit-hostnet/src/networkd/mod.rs)
- 说明：配置目录存在时把 `[Match] Name=` 精确集 ⊆ 选中集的 `.network` 文件整体移出，
  备份落 `backups/hostnet/networkd`；文件移出后对运行中的 networkd `networkctl reload`，
  工具缺失或 reload 失败中止安装；服务全程零变更 systemctl 调用。回滚/卸载按 manifest
  逐字重建移出文件并再次 reload。摘除现场与 NM/firewalld/ifupdown 可组合（NET-13、
  NET-14），reinit 反查并入 `Name=` 精确集（REI-11）。

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
- 说明：覆盖手工 rollback、10 分钟 timer rollback 和确认前重启的 boot rollback；三条入口
  都必须恢复宿主网络（含按 `backups/hostnet/<适配器>` 的 manifest 逐字恢复摘除——
  ifupdown 原文件、firewalld zone、networkd 移出文件重建、NM drop-in 删除——并在服务
  active 时 restart `networking.service`、reload 运行中的 NM/firewalld/networkd）、
  删除未提交首次安装的整个 `data/`，并允许随后带新凭据重新执行
  `lkit install`。
- 缺口：fixture 直接覆盖自动回滚入口和重装，QEMU 覆盖 boot rollback；真实 timer 到期和
  手工 systemd operation worker 尚未分别触发。

## NET-11

**网络接管回滚清理失败时保留现场并进入 failed，不伪造 rolled_back**

- 测试层：CLI fixture E2E、Rust 事务测试
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/)、[事务与中断恢复](../../../deployment/transactions-and-recovery.md#未提交网络接管安装的回滚清理)
- 说明：fixture 通过异常 `current` 链接注入清理失败，断言退出码 `6`、事务为 `failed` 且
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
- 说明：`networking.service` 与 systemd-networkd 都是受支持的摘除目标（NET-13、
  NET-15），不属于未知管理器；未知的活动网络管理器只剩 wicked 与 connman。已有
  `br_lan` 不阻断安装：install 与 reinit 都不检查桥接是否存在，桥接的
  创建、成员同步与清理由 Landscape 按新配置处理。
