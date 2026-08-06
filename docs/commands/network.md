# `lkit network`

网络子命令只处理 `lkit install --takeover-network` 创建的待定事务：

```text
lkit network status [--install-dir <PATH>]
lkit network confirm [--install-dir <PATH>]
lkit network rollback [--install-dir <PATH>]
```

- `status` 显示事务阶段、管理地址和确认截止时间。
- `confirm` 必须从重新连接到目标管理 IPv4 的 SSH 会话运行。它重新核对接口 MAC、目标
  地址、`br_lan` 成员、Landscape MainPID 和健康检查，再提交安装状态并移除恢复 unit。
- `rollback` 清理未提交的首次安装并按事务快照恢复 NetworkManager、`networking.service`、
  firewalld 和 systemd-resolved 的 enabled/active/masked 状态。手工 rollback 由 systemd
  operation worker 执行，恢复网络服务导致当前 SSH 断开也不会中止后续恢复。

确认不是“安装进程仍能访问新 IP”或 ICMP ping。只有新 SSH 会话中的
`SSH_CONNECTION` 服务端地址与计划管理地址一致才可确认。
