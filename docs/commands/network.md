# `lkit network`

网络子命令处理首次接管、reinit 和 restore 的网络确认事务：

```text
lkit network status
lkit network confirm
lkit network rollback
```

- `status` 显示事务阶段与确认截止时间。首次接管和 reinit 显示计划中的管理地址；
  restore 提示使用备份中的管理地址，不从任意备份配置推测固定地址。
- `confirm` 在用户验证应用后的网络可达后执行。首次接管和 reinit 复检接口 MAC、
  目标地址、bridge 成员、Landscape PID 和健康；restore 复核待提交安装身份、活动
  二进制、服务后端、PID 和健康。期限及主机启动标识复核通过后，进入 `finalizing`
  并提交状态。restore 的 `--yes` 和本机健康检查均不能代替这一步。
- `rollback` 恢复未确认变更之前的配置与服务状态；首次安装清理未提交的安装数据，
  reinit 恢复原 Landscape 数据和状态，restore 恢复原 release、data、state、服务及
  DNS 文件。首次安装接管新建的 `br_lan` 时，同次开机回滚会移除接管配置的管理 IPv4 地址，让内核撤销对应的直连路由。
  失败保留 `rolling_back`，由 daemon 重试。
- 手工确认与回滚由 daemon 直接执行；CLI 提交绑定安装根与事务 ID 的结构化操作，
  输出操作 ID 后观察结果。SSH 断开或 Ctrl+C 只退出观察，操作继续执行。
- 请求仅排队时可取消。领取后与自动恢复共用安装锁；网络操作优先于等待中的普通修改
  请求，不能隐式确认其他事务。事务已变更时拒绝旧请求。
- `lkit operation status <id>` 查询持久化结果。确认以事务提交为成功依据，回滚以
  `rolled_back` 为成功依据。确认未获接受时失败或中断，不自动重放用户确认；已进入
  `finalizing` 或 `rolling_back` 时由 daemon 继续恢复并更新结果。
- daemon 在确认期限到达或检测到主机重启时自动回滚；daemon 自身重启继续原期限。
  已接受确认的事务继续提交，不自动撤销确认。

服务定义使用内部 `network startup-check` 检查启动条件。待确认事务过期或跨主机
启动时，目标配置被阻止再次启动；回滚只在原配置恢复完成并取得当前启动的放行凭据后
启动原服务。该检查不替用户确认网络可用。

完整契约见 [daemon 网络确认与恢复](../network/daemon-recovery.md)。
