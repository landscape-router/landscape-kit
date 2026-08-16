# 网络接管场景

## NET-01

**双网口安装选择 LAN 时生成 br_lan route 与 DHCP**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络配置测试](../../../../crates/lkit-cli/src/network/config.rs)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：仅当用户至少选择一个 LAN 时创建 `br_lan`；空 LAN 使用 WAN-only 计划。

## NET-09

**多网口 CLI 允许 LAN 为空并按 WAN-only 计划安装**

- 测试层：Rust 单元、CLI 交互
- 状态：`已覆盖`
- 证据：[网络发现](../../../../crates/lkit-cli/src/network/discovery.rs)、[网络配置](../../../../crates/lkit-cli/src/network/config.rs)
- 说明：LAN 选择提示接受空输入；空集合转换为 `WanOnly`，不创建 `br_lan` 或 LAN DHCP。选择一个或多个 LAN 时仍使用 RoutedLan。

## NET-10

**WAN IPv4 配置与所选 LAN 地址清理遵循网络计划**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络配置](../../../../crates/lkit-cli/src/network/config.rs)、[网络发现](../../../../crates/lkit-cli/src/network/discovery.rs)、[网络接管](../../../../crates/lkit-cli/src/network/takeover.rs)
- 说明：CLI 发现完整地址/网关时取所选 WAN 的第一个 IPv4 作为静态配置，否则使用 DHCP；
  停止宿主网络服务后只清理所选 LAN 的 IPv4/IPv6 地址。

## NET-02

**单网口保留 SSH IPv4/网关并为 TCP 22、6443 创建 Local 静态映射**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[网络配置测试](../../../../crates/lkit-cli/src/network/config.rs)

## NET-03

**接口始终由用户选择，选择结果与 MAC 写入事务供确认复核**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[接口发现](../../../../crates/lkit-cli/src/network/discovery.rs)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

## NET-04

**恢复 timer 与 boot rollback unit 在停止宿主网络服务前持久化并启动**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

## NET-05

**NetworkManager 或 Debian ifupdown 的 `networking.service`、firewalld、systemd-resolved 被停止、disable、mask，但软件包不卸载**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：覆盖 NetworkManager 缺失且 `networking.service` 处于 active/enabled 状态的接管与回滚。

## NET-06

**任意可达会话均可确认并提交安装，TUI 以待确认阻塞屏提示**

- 测试层：CLI fixture E2E、QEMU/KVM、Ratatui TestBackend
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
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
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)
- 说明：覆盖手工 rollback、10 分钟 timer rollback 和确认前重启的 boot rollback；三条入口
  都必须恢复宿主网络、删除未提交首次安装的整个 `data/`，并允许随后带新凭据重新执行
  `lkit install`。
- 缺口：fixture 直接覆盖自动回滚入口和重装，QEMU 覆盖 boot rollback；真实 timer 到期和
  手工 systemd operation worker 尚未分别触发。

## NET-11

**网络接管回滚清理失败时保留现场并进入 failed，不伪造 rolled_back**

- 测试层：CLI fixture E2E、Rust 事务测试
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[事务与中断恢复](../../../deployment/transactions-and-recovery.md#未提交网络接管安装的回滚清理)
- 说明：fixture 通过异常 `current` 链接注入清理失败，断言退出码 `6`、事务为 `failed` 且
  残留 data 未被删除。

## NET-08

**SELinux 与不受支持的活动网络管理器在任何网络变更前阻断**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[接口发现](../../../../crates/lkit-cli/src/network/discovery.rs)
- 说明：`networking.service` 是受支持的 Debian ifupdown 宿主服务，不属于本场景的未知
  管理器。已有 `br_lan` 不阻断安装：install 与 reinit 都不检查桥接是否存在，桥接的
  创建、成员同步与清理由 Landscape 按新配置处理。
