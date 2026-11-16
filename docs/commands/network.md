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
- 手工确认与回滚由 daemon 直接执行；CLI 提交绑定安装根与事务 ID 的结构化操作，
  输出操作 ID 后观察结果。SSH 断开或 Ctrl+C 只退出观察，操作继续执行。
- 请求仅排队时可取消。领取后与自动恢复共用安装锁；网络操作优先于等待中的普通修改
  请求，不能隐式确认其他事务。事务已变更时拒绝旧请求。
- `lkit operation status <id>` 查询持久化结果。确认以事务提交为成功依据，回滚以
  `rolled_back` 为成功依据。确认未获接受时失败或中断，不自动重放用户确认；已进入
  `finalizing` 或 `rolling_back` 时由 daemon 继续恢复并更新结果。
- daemon 在确认期限到达或检测到主机重启时自动回滚；daemon 自身重启继续原期限。
  已接受确认的事务继续提交，不自动撤销确认。

服务定义使用内部 `network startup-check` 检查启动条件。该检查阻止 Landscape
在跨主机启动的未确认接管恢复前启动；它不替用户确认网络可用。

完整契约见 [daemon 网络接管恢复](../network/daemon-recovery.md)。
