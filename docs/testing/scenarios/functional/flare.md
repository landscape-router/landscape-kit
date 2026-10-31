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
