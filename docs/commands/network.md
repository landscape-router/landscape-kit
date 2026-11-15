# `lkit network`

网络子命令处理首次接管和 reinit 的网络事务：

```text
lkit network status
lkit network confirm
lkit network rollback
```

- `status` 显示事务阶段、管理地址与确认截止时间。
- `confirm` 在任意可达会话执行，复检接口 MAC、目标地址、bridge 成员、Landscape PID
  和健康状态后接受用户确认，进入 `finalizing` 并提交状态。
- `rollback` 恢复未确认接管之前的配置与服务状态；首次安装清理未提交的安装数据，
  reinit 恢复原 Landscape 数据和状态。失败保留 `rolling_back`，由 daemon 重试。
- 手工确认与回滚通过 daemon worker 执行，SSH 断开不终止操作。
- daemon 在确认期限到达或检测到主机重启时自动回滚；daemon 自身重启继续原期限。
  已接受确认的事务继续提交，不自动撤销确认。

服务定义使用内部 `network startup-check` 检查启动条件。该检查阻止 Landscape
在跨主机启动的未确认接管恢复前启动；它不替用户确认网络可用。

完整契约见 [daemon 网络接管恢复](../network/daemon-recovery.md)。
