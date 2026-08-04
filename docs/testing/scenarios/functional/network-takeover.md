# 网络接管场景

## NET-01

**双网口安装由用户选择 WAN/LAN 并生成 br_lan route 与 DHCP，不设置 WAN IP 或 NAT**

- 测试层：Rust 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[网络配置测试](../../../../crates/lkit-cli/src/network/config.rs)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

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

**NetworkManager、firewalld、systemd-resolved 被停止、disable、mask，但软件包不卸载**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

## NET-06

**只有从新管理地址重新建立的 SSH 会话能确认并提交安装**

- 测试层：CLI fixture E2E、QEMU/KVM
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)

## NET-07

**未确认回滚清理安装并精确恢复三个宿主服务状态**

- 测试层：CLI fixture E2E、QEMU/KVM
- 状态：`部分覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)、[QEMU 网络接管](../../qemu-network-takeover.md)

## NET-08

**SELinux、已有 br_lan 或未知活动网络管理器在任何网络变更前阻断**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[接管预检](../../../../crates/lkit-cli/src/network/takeover.rs)、[接口发现](../../../../crates/lkit-cli/src/network/discovery.rs)
