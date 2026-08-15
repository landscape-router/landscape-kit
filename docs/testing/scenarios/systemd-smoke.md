# systemd 兼容性 Smoke 场景

本文件只列 fake systemctl 无法证明的真实 manager 契约。它不重新执行发布、首次安装、
切换、备份、回滚、repair 或 reconcile 场景，不参与核心功能覆盖率，也不是普通 PR 或
普通发布门禁。仅在 unit、daemon worker 委托或 systemctl 适配变化时按需运行，CI 当前还会
每周执行一次。

## SYS-01

**unit 可被真实 systemd manager 接受并完成注册、启动、停止和注销**

- 测试层：systemd-nspawn
- 状态：`低频 smoke`
- 证据：[nspawn 兼容性 smoke](../nspawn-systemd.md)

## SYS-02

**服务启动后真实 manager 报告非零 MainPID**

- 测试层：systemd-nspawn
- 状态：`低频 smoke`
- 证据：[nspawn 兼容性 smoke](../nspawn-systemd.md)

## SYS-03

**CLI 前端断开后委托操作由 daemon 继续提交事务**

- 测试层：systemd-nspawn
- 状态：`低频 smoke`
- 说明：当前 smoke 直接验证提交路径；断连后的回滚路径不属于当前抽样范围，见 [nspawn 兼容性 smoke](../nspawn-systemd.md)。

## SYS-04

**真实 manager 存在 foreign unit 时接管失败，失败状态不残留**

- 测试层：systemd-nspawn
- 状态：`低频 smoke`
- 证据：[nspawn 兼容性 smoke](../nspawn-systemd.md)

## SYS-05

**KVM 虚拟机中停止宿主网络服务后 Landscape 创建 br_lan，SSH 可从新地址重新连接并确认**

- 测试层：QEMU/KVM
- 状态：`低频 smoke`
- 证据：[QEMU 网络接管](../qemu-network-takeover.md)

## SYS-06

**KVM 虚拟机中未确认超时或重启会恢复接管前网络及宿主服务**

- 测试层：QEMU/KVM
- 状态：`低频 smoke`
- 证据：[QEMU 网络接管](../qemu-network-takeover.md)
