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

**只有从新管理地址重新建立的 SSH 会话能确认并提交安装**

- 测试层：CLI fixture E2E、QEMU/KVM
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)
- 说明：双网口在确认前保留继承的 WAN IPv4，确认检查通过后清除；清除失败不提交。

## NET-07

**未确认回滚清理安装并精确恢复宿主网络服务状态**

- 测试层：CLI fixture E2E、QEMU/KVM
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)

## NET-08

**SELinux、已有 br_lan 或不受支持的活动网络管理器在任何网络变更前阻断**

- 测试层：CLI fixture E2E、Rust 单元
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[接口发现](../../../../crates/lkit-cli/src/network/discovery.rs)
- 说明：`networking.service` 是受支持的 Debian ifupdown 宿主服务，不属于本场景的未知管理器。
