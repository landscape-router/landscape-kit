# `lkit service-manager`

在现有安装的运行管理模式之间迁移。

```text
lkit service-manager systemd [--install-dir <PATH>]
lkit service-manager none [--install-dir <PATH>]
```

服务管理器操作通过 [`ServiceManager` trait](../service/manager.md) 抽象；
v1 只实现 systemd 后端，`--service-manager` 可选值固定为 `systemd` 或 `none`，
未来后端接入后扩展该选项。

## Service manager 迁移

service manager 迁移只改变 Landscape 的进程管理方式，不下载版本资产、不修改 `current`、
不修改 Landscape data，也不创建 `.lkb`。目标模式与当前模式相同时成功返回且不执行迁移；
实际迁移必须创建独立的 `service_migration` 事务并持有安装锁。

### systemd → none

用户对当前 `service.manager: "systemd"` 的安装显式执行 `--service-manager none` 时：

1. 验证当前 unit 原件和系统注册链接仍满足受管所有权与安全规则；
2. 创建 `preparing` 事务，记录 `systemd_before`；本迁移不修改 `/etc/resolv.conf`，因此 `resolv_conf_backup` 为 null；
3. 将事务更新为 `prepared`；
4. 在 stop 前将事务更新为 `stopping`；
5. 停止当前服务并确认受管进程退出；
6. 将事务更新为 `activating`；
7. disable 服务，移除受管系统注册链接并执行 `daemon-reload`；受管 unit 原件保留在 `<install-root>/service/`；
8. 原子提交状态中的 service 对象为 `manager: "none"`、`registered: false`、`enabled: false`、`verified: false`、`definition_path: null`、`definition_sha256: null`；
9. 将事务更新为 `committed`，输出参考启动命令，但不启动 Landscape。

停止服务之后的任一步失败时，按 `systemd_before` 恢复注册链接和 enabled/active 状态。恢复成功仍返回普通失败；恢复失败则将事务标为 `failed` 并要求人工恢复。

迁移成功后 Landscape 处于停止状态，何时以及如何以外部方式启动由用户负责。

### none → systemd

用户对当前 `service.manager: "none"` 的安装显式执行 `--service-manager systemd` 时：

1. 完整验证 systemd 可用性、当前版本后端摘要、初始化状态和受管目录安全性；`initialization.status` 可以是 `pending` 或 `complete`，但本文定义的初始化锁缺失高危异常仍必须阻断；
2. 检查导出和数据备份不参与本迁移；`lkit` 不修改 `current` 或 data；
3. 要求用户先按自己的方式停止外部 Landscape，并通过 `/dev/tty` 输入 `yes` 确认；无 `/dev/tty` 时阻断，v1 不支持无人值守接管；
4. 确认固定端口 `53`、`6300` 和 `6443` 已释放，存在无法确认的占用时阻断；
5. 创建 `preparing` 事务，记录 `systemd_before`；正常情况下注册链接为 missing、enabled 和 active 为 false；
6. 备份 `/etc/resolv.conf` 并写入 `resolv_conf_backup`，生成或验证受管 unit 原件，将事务更新为 `prepared`；
7. 将事务更新为 `activating`，创建系统注册链接并执行 `daemon-reload`、enable 和 start；
8. 目标进程创建后更新为 `verifying`，执行完整的 180 秒启动检查和 10 秒稳定观察；迁移前初始化为 `pending` 时，同时要求生成 `landscape_init.lock` 和 `landscape.toml`；
9. 原子提交状态中的 service 对象为 `manager: "systemd"`、`registered: true`、`enabled: true`、`verified: true`，并记录 unit 原件路径和摘要；初始化由 `pending` 完成时，同时提交 `initialization.status: "complete"`、`lock_present: true` 和本次首次观察时间；
10. 将事务更新为 `committed`。

注册或启动失败时停止本次 systemd 服务，恢复 `systemd_before` 和 `/etc/resolv.conf`，并保持已提交状态为 `manager: "none"`。`lkit` 不知道用户原来的外部启动命令，因此不会尝试重新启动外部实例；恢复成功返回普通失败并明确提醒 Landscape 当前未运行，恢复失败则标记 `failed` 并要求人工恢复。

两个迁移方向在生产模式下都由临时 systemd operation unit 执行。SSH 前端断开后迁移
继续完成或回滚；主机重启则由下一次 lkit 调用恢复未结束事务。
