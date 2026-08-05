# `lkit switch`

将现有安装切换到更高的 stable 版本。目标版本必须按 SemVer 高于当前活动版本；目标版本
更低时返回参数用法错误，不创建切换事务，也不下载目标二进制和静态资产；目标版本相同
时沿用既有的同版本安装校验，不执行版本切换。降级限制同时适用于精确版本和 `latest`
解析出的版本。

```text
lkit switch --version <VERSION> [--repository [<BASE_URL>]]
            [--install-dir <PATH>] [--accept-service-change]
            [--allow-no-backup]
```

允许升级时，目标资产必须在停止当前服务前完成下载和校验。systemd 环境由 `lkit` 停止、激活、
启动并验证。正常路径在停止服务前创建 `.lkb`，失败时用它重建旧版本；服务已经停止且
用户显式指定 `--allow-no-backup` 时是唯一例外，此时仍恢复文件、服务状态、`current`
和 `/etc/resolv.conf`，但无法从快照重建 data。无 systemd 环境要求用户确认已通过自己的
进程管理器停止实例。

生产 systemd 环境中，整条 switch 命令由临时 systemd operation unit 托管。SSH 会话
因 Landscape 重启而断开时，worker 仍会继续健康检查并提交，或在失败时自动回滚。
事务在 stop 前先写入 `stopping`；主机重启不自动继续，而由下次调用恢复。

未指定 `--repository` 时沿用 state 记录的仓库。显式指定其他仓库即表示本次切换使用
该来源，不再进行第二次确认；只有切换成功后才把新来源写入 state。

初始化完成后，switch 不读取、比较或改写现场保留的
`data/landscape_init.toml`。该文件内容变化或删除不需要接受参数；初始化锁缺失仍属于
不可绕过的高危状态。

## 停止服务后的切换

配置导出 API 由运行中的服务提供，`.lkb` 快照也依赖它。systemd 环境下受管服务已停止时，切换默认**拒绝**：

```text
install: the managed service is not running: the managed service is stopped;
start it with `systemctl start landscape-router.service` and retry, or re-run
with --allow-no-backup to switch without a configuration snapshot
```

用户必须先启动服务再重试。显式 `--allow-no-backup` 时继续切换，但必须清楚后果：

- 不查询配置导出 API、不创建 `.lkb`，事务记录 `no_backup: true` 且无 `backup`；
- 目标版本激活失败时仍自动回滚到旧版本（停止目标进程、恢复 unit 注册与
  enabled/active 状态、恢复 `/etc/resolv.conf` 与 `current` 链接，
  切换前服务在运行时才重新启动旧版本），但**无法恢复被目标版本重新初始化过的数据**；
- 服务正在运行且没有停止时，`--allow-no-backup` 被忽略并输出警告，
  仍然创建 `.lkb`。
