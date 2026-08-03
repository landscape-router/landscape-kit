# 失败切换与自动回滚

## 前置状态

该场景从已经成功运行的 `2.0.0` 开始。在 `landscape.toml` 中写入第二个唯一标记，用于证明导出配置进入 `.lkb` 并在回滚后恢复。

## 失败目标

`3.0.0` 使用 `health_error` 场景：

- 可以启动并绑定生产端口；
- HTTPS `/api/docs` 返回非成功状态；
- lkit 的启动健康检查失败；
- lkit 仍生成并校验生产 unit 的 `Restart=always` 内容；进程启停由 fake systemctl
  按 systemctl 协议执行。

执行：

```sh
lkit switch \
  --version 3.0.0 \
  --install-dir /var/lib/lkit-e2e/landscape
```

## 必须断言

- 命令返回现有“切换失败但自动回滚成功”退出码 `5`；
- 切换前创建的 `.lkb` metadata 中 `landscape_version` 为 `2.0.0`；
- 最新 switch 事务最终 phase 为 `rolled_back`；
- active version 和 `current` 恢复为 `2.0.0`；
- `2.0.0` release 从 `.lkb` 中的 binary 和 static 内容重建；
- 运行进程 SHA 恢复为备份内的 `2.0.0` 后端 SHA；
- `data/landscape_init.toml` 包含第二个用户标记；
- fixture 从恢复后的 init 文件重新创建的 `landscape.toml` 也包含该标记；
- fake systemctl 状态恢复 enabled 和 active；
- 测试运行时隔离的 `resolv.conf` 内容和权限与切换前完全一致；
- 手动 restart `landscape-router.service` 后仍通过进程身份和 HTTPS 健康检查。

## 备份边界

minimal `.lkb` 不包含 `landscape_db.sqlite`。该场景不能声称数据库内容得到恢复，只验证：

- 后端 binary；
- static；
- 导出的 init config；
- geo cache；
- install state；
- systemd service-manager 协议状态；
- 测试运行时隔离的 `resolv.conf`。
