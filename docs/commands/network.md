# `lkit network`

网络子命令只处理 `lkit install --takeover-network` 创建的待定事务：

```text
lkit network status [--install-dir <PATH>]
lkit network confirm [--install-dir <PATH>]
lkit network rollback [--install-dir <PATH>]
```

- `status` 显示事务阶段、管理地址和确认截止时间。
- `confirm` 不限制 SSH 会话来源，在任意可达主机的会话（包括本地控制台）中都能运行。
  它重新核对接口 MAC、目标地址、`br_lan` 成员、Landscape MainPID 和健康检查，再提交安装
  状态并移除恢复 unit。
- `rollback` 清理未提交的首次安装并按事务快照恢复 NetworkManager、`networking.service`、
  firewalld 和 systemd-resolved 的 enabled/active/masked 状态。手工 rollback 由 systemd
  operation worker 执行，恢复网络服务导致当前 SSH 断开也不会中止后续恢复。
- 确认前主机重启、10 分钟确认 timer 到期和手工 `rollback` 都使用同一幂等回滚入口；重启
  不会继续保留确认窗口，而是按未确认处理。
- 只有确认成功才会提交 `state/install-state.json`。回滚成功后删除未提交首次安装的
  `current`、目标 release、pending state 和整个 `data/`，安装根目录恢复为可重新首次安装
  的状态；事务 JSON 和日志保留用于审计。
- 回滚只接受 `install` 的 `awaiting_network_confirmation`、`finalizing` 或
  `rolling_back` 事务。任何恢复或清理失败都会进入 `failed` 并要求人工处理，不报告
  `rolled_back`。

确认不是“安装进程仍能访问新 IP”或 ICMP ping。它依据主机本地的复检：接口 MAC、
管理 IPv4/prefix 与 `br_lan` 成员必须与计划一致，Landscape 必须健康。推荐从新管理地址
重新连接后运行（停止宿主网络服务会断开旧会话），但不作为强制校验。
