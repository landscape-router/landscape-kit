# Landscape Terrain 协议栈调度

协议与真实链路场景 `FLR-01` 至 `FLR-32` 见 [flare 场景](../../../flare/scenarios.md)。

## FLR-33

**空闲协议栈没有周期性轮询定时器**

- 测试层：纯内存单元测试、Tokio 模拟时间
- 状态：`已覆盖`
- 证据：[协议栈](../../../../landscape-terrain-proto/src/ipstack.rs)
  （`idle_and_listening_stacks_have_no_deadline`）、
  [调度等待](../../../../landscape-terrain-proto/src/relay.rs)
  （`idle_has_no_protocol_timer`、`waits_for_the_exact_deadline`）、
  [服务端](../../../../lkit-cli/src/flare/server.rs)
  （`deadlines_select_only_due_peers_and_teardown_clears_them`）。
- 说明：覆盖无连接、只有监听 socket、已建立但无数据的 TCP 连接。keepalive 与 stale
  清扫保持独立，不属于协议栈轮询定时器。

## FLR-34

**无新入站数据时 TCP 定时任务仍能推进**

- 测试层：纯内存协议栈、显式推进单调时间
- 状态：`已覆盖`
- 证据：[协议栈](../../../../landscape-terrain-proto/src/ipstack.rs)
  （`lost_syn_retransmits_at_deadline_without_ingress`、
  `data_delayed_ack_and_fin_advance_using_only_deadlines`）、
  [调度规范](../../../flare/protocol.md#隧道与端口转发)。
- 说明：仅按下一截止时间和待处理入站数据推进，验证丢失 SYN 后重传、延迟 ACK、FIN
  和 TIME-WAIT 到期；限制同一时间点的推进次数以检测零延迟忙循环。

## FLR-35

**慢消费者恢复容量或退出时唤醒协议栈，旧连接通知不影响新连接**

- 测试层：有界 channel 与内存 TCP 栈单元测试
- 状态：`已覆盖`
- 证据：[容量通知](../../../../landscape-terrain-proto/src/relay.rs)
  （`capacity_and_receiver_drop_wake_without_data_channel_space`、
  `wakes_coalesce_and_keep_connection_generations_distinct`、
  `cancelled_wait_does_not_lose_later_progress`）、
  [TCP 窗口恢复](../../../../landscape-terrain-proto/src/ipstack.rs)
  （`slow_reader_reopens_tcp_window_without_periodic_ticks`）、
  [服务端代号过滤](../../../../lkit-cli/src/flare/server.rs)
  （`capacity_wakes_ignore_stale_connections_and_coalesce_peers`）。
- 说明：通知独立于有容量上限的数据路径，消费者读取后、执行可能阻塞的 socket 写入前
  通知；退出时先关闭接收端再通知。多次通知按连接合并，取消等待不会丢失后续通知。

## FLR-36

**会话销毁取消本地转发任务，正常 TCP 半关闭仍保留反向响应**

- 测试层：Rust 隔离单元、临时 loopback 端口
- 状态：`已覆盖`
- 证据：[转发任务生命周期](../../../../lkit-cli/src/flare/server.rs)
  （`disappearing_relay_channel_closes_the_local_socket`、
  `session_teardown_cancels_half_closed_relays`）、
  [转发规范](../../../flare/protocol.md#隧道与端口转发)。
- 说明：本地服务保持连接且不主动关闭时，channel 消失、会话销毁和服务端退出均使
  转发任务退出。显式 `PeerEof` 只关闭写方向，仍能转发本地服务的响应；之后销毁会话
  也能取消已经半关闭的任务，避免 Flare 重载后遗留 socket。

## FLR-37

**同一网段的两台同名设备可以分别选中**

- 测试层：containerlab e2e（CI）
- 状态：`已覆盖`
- 证据：[三节点场景](../../../../scripts/flare/e2e-containerlab.sh)、[拓扑](../../../../scripts/flare/containerlab/rescue.clab.yml)
- 说明：延迟一侧响应，按两个指纹分别建立映射，HTTP 响应标记 A/B。扫描不建立会话。

## FLR-38

**本地密码认证与端口服务认证独立**

- 测试层：containerlab e2e（CI）
- 状态：`已覆盖`
- 证据：[三节点场景](../../../../scripts/flare/e2e-containerlab.sh)、[拓扑](../../../../scripts/flare/containerlab/rescue.clab.yml)
- 说明：Debian 与 Alpine 的 root 密码可用，锁定后拒绝。无 sshd 时 HTTP 可用。SSH 映射内验证无密钥拒绝、持正确密钥成功。

## FLR-39

**Landscape 晚安装及改密**

- 测试层：containerlab 配置夹具、官方 Landscape 0.28.3 的 Debian QEMU e2e
- 状态：`已覆盖`
- 证据：[三节点场景](../../../../scripts/flare/e2e-containerlab.sh)、[官方安装与改密](../../../../scripts/qemu/network/flare-rescue.sh)、[QEMU 实测结果](https://github.com/landscape-router/landscape-kit/actions/runs/44377715440/job/135061210605)
- 说明：daemon 先于安装启动。通过真实管理员 API 改密，验证新密码可登录 Web，停止 Web 后分别用旧、新密码连接 Flare。检查旧密码被明确拒绝、新密码能映射 SSH、systemd 的 daemon 启动实例保持一致。

## FLR-40

**私有发现与旧版互通**

- 测试层：containerlab e2e（CI）
- 状态：`已覆盖`
- 证据：[三节点场景](../../../../scripts/flare/e2e-containerlab.sh)、[拓扑](../../../../scripts/flare/containerlab/rescue.clab.yml)
- 说明：未持正确 PSK 的客户端发现不了私有设备。目标离线时连接失败。新客户端使用旧恢复码与固定版本的真实 v5 daemon 完成 HTTP 转发并提示升级。

## FLR-41

**取消设备确认后返回选择列表，重新连接需再次确认**

- 测试层：containerlab 内的真实 TUI、伪终端和隔离网卡，另有 Noise 内存单测
- 状态：`已覆盖`
- 证据：[TUI 验收](../../../../scripts/flare/containerlab/tui-confirmation.py)、[Noise 握手](../../../../landscape-terrain-proto/src/protocol/v6.rs)
- 说明：连续两次取消完整指纹确认，验证界面返回设备列表且没有发出认证帧。第三次确认并提交密码，验证能够观察到认证帧并进入已连接界面。握手对端密钥与所选设备不匹配时，客户端无法取得传输通道。

## FLR-42

**Windows 上的 Npcap 客户端按指纹选择两台设备**

- 测试层：Windows、已安装的 Npcap、隔离以太网
- 状态：`待实机验证`
- 证据：[PowerShell 7 验收脚本](../../../../scripts/flare/windows-npcap-smoke.ps1)、[Windows QEMU 场景](../../../../scripts/flare/windows-qemu.sh)、[CI 镜像约定](../../../flare/windows-ci.md)
- 说明：两台测试路由器启用公开发现和 root 密码认证，各自的本地 HTTP 服务在允许映射的端口上返回单独一行 A 或 B（默认端口 6443）。从路由器控制台取各自完整指纹。Windows 测试机上按以下方式运行，脚本在每次读取 HTTP 标记后关闭对应客户端。

```powershell
./scripts/flare/windows-npcap-smoke.ps1 -Executable .\lflare.exe `
  -Interface '\Device\NPF_{测试网卡GUID}' `
  -FingerprintA $routerA -PasswordFileA .\a-password.txt `
  -FingerprintB $routerB -PasswordFileB .\b-password.txt
```
