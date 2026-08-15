# `lkit self-service`

把 lkit 自身安装为受管服务（lkit 常驻服务化的安装入口）。

```text
lkit self-service install [--install-dir <PATH>]
lkit self-service remove [--install-dir <PATH>]
```

重复执行 `install` 时：若旧 daemon 仍在运行，注册完成后执行 `restart`
使其加载新二进制；二进制复制会跳过「源与目标相同」的情况（从
`<root>/service/lkit` 运行 install 时不会自毁）。注册或启动失败时尽力
回滚已注册状态并删除二进制与定义原件。

## install

1. 解析安装根目录（`--install-dir` / `LKIT_INSTALL_DIR` / 默认
   `/root/.lkit/landscape`）并获取安装锁；
2. 服务管理器固定为 systemd；
3. 创建 `<root>/service/` 与 `<root>/data/`；
4. 把当前 lkit 可执行文件复制到 `<root>/service/lkit`（权限 `0700`，
   与网络接管恢复二进制的存放约定一致）；
5. 通过 [`ServiceManager`](../service/manager.md) trait 的
   `LkitDaemon` 定义渲染 `<root>/service/lkit.service`：
   `ExecStart=<root>/service/lkit daemon --config-dir <root>/data`，
   `User=root`、`Restart=always`、`WantedBy=multi-user.target`；
6. 注册、启用并启动服务，校验 MainPID 非零。

## remove

停止并等待 lkit daemon 退出，注销并移除注册与定义原件，删除
`<root>/service/lkit` 二进制。命令可重复执行（幂等）。

## daemon 进程

`lkit daemon --config-dir <root>/data` 是常驻服务本体：

- pidfile 写入 `<root>/run/lkit.pid`（`0600`，原子替换）；已存在存活实例时拒绝启动；
- 收到 `SIGTERM` / `SIGINT` 后清理 pidfile 并退出；
- **周期中断恢复**：每 2 秒尝试以非阻塞方式获取安装锁，锁空闲且存在未完成
  事务时，执行与 CLI 相同的 `recover_interrupted` 语义——CLI 因 SSH 断开或
  崩溃消失后，遗留事务由 daemon 自动接管（失败激活回滚、中断恢复、卸载前向
  完成等，详见[事务与中断恢复](../deployment/transactions-and-recovery.md)）；
- 恢复目标固定为 daemon 自身所在的安装根；网络接管待确认阶段仍由
  `lkit network confirm|rollback` 人工处理，daemon 不代替确认；
- CLI 命令持有安装锁期间 daemon 自动让行，不产生并发冲突。

## 卸载与常驻服务

`lkit uninstall` 会同时停止并注销 `LkitDaemon` 服务（若已安装），
不会遗留 `lkit.service` 注册或运行中的 daemon。

## 退出码

- `0`：成功；
- `2`：参数错误（如请求 systemd 但不可用）；
- `1`：其他失败。
