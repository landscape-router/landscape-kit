# systemd 兼容性 Smoke 场景

本文件只列 fake systemctl 无法证明的真实 manager 契约。它不重新执行发布、首次安装、
切换、备份、回滚、repair 或 reconcile 场景，不参与核心功能覆盖率，也不是普通 PR 或
普通发布门禁。仅在 unit、daemon 原生操作或 systemctl 适配变化时按需运行，CI 当前还会
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

**CLI 前端断开后 daemon 继续提交原生操作**

- 测试层：systemd-nspawn
- 状态：`低频 smoke`
- 证据：[nspawn 兼容性 smoke](../nspawn-systemd.md)
- 说明：smoke 覆盖提交与回执（S-1）、前端断开后独立完成（S-2）、运行中拒绝取消及崩溃恢复（S-3）、daemon 不可用时拒绝（S-4）、二进制路径删除后继续执行（S-4b）、操作语言（S-5）。

## SYS-04

**真实 manager 存在 foreign unit 时接管失败，失败状态不残留**

- 测试层：systemd-nspawn
- 状态：`待补充`
- 说明：nspawn 兼容性 smoke 当前只抽样 unit 注册、启停与 daemon 操作契约，尚未包含
  foreign unit 所有权冲突的接管失败路径（见 [nspawn 兼容性 smoke](../nspawn-systemd.md)）。

## SYS-05

**KVM 虚拟机中停止宿主网络服务后 Landscape 创建 br_lan，SSH 可从新地址重新连接并确认**

- 测试层：QEMU/KVM
- 状态：`低频 smoke`
- 证据：[QEMU 网络接管](../qemu-network-takeover.md)

## SYS-06

**KVM 虚拟机中未确认重启会恢复接管前网络及宿主服务**

- 测试层：QEMU/KVM
- 状态：`低频 smoke`
- 证据：[QEMU 网络接管](../qemu-network-takeover.md)

- 缺口：真实虚拟机中的确认超时回滚；当前脚本直接验证重启回滚，超时决策由单元与 fixture 测试覆盖。

## SYS-07

**真实 networkd 接管与原始启动状态恢复**

- 测试层：systemd-nspawn CI
- 状态：`已覆盖`
- 证据：[nspawn 入口](../../../scripts/test-nspawn-systemd.sh)、[networkd 场景](../../../scripts/test-nspawn-networkd.py)
- 说明：在隔离网络的 Debian guest 中运行真实 networkd、systemctl 和 lkit daemon。服务启用且运行、socket 禁用且停止、wait-online runtime mask，以及无底层 unit 文件的预设 mask；首次安装接管后检查屏蔽及启动链接（含指向 mask 路径的相对链接），验证 D-Bus 别名与 socket 无法拉起服务，重启 daemon 后回滚并核对原始链接及运行状态。网络发现与 Landscape 使用 fixture，不在宿主机运行。

## SYS-08

**受管 Landscape 回滚保留仅运行时启用状态**

- 测试层：Debian/systemd QEMU、隔离文件与序列化单元
- 状态：`部分覆盖`
- 证据：[真实恢复故障链 JNY-04](functional/real-install-recovery.md#jny-04)、[启用恢复实现](../../../lkit-cli/src/service/systemd_enablement.rs)、[快照格式](../../../lkit-cli/src/service/manager_enablement.rs)
- 说明：真实 manager 将 Landscape 设为仅运行时启用后执行 restore 与主动回滚，
  检查运行时成员及业务仍在，未转换为永久启用。单元覆盖持久与运行时成员混合、
  整机换 boot ID 后临时成员到期、外部链接拒绝和兼容快照读取。
- 缺口：真实 manager 上混合成员及运行时成员跨整机重启到期的独立验收。
