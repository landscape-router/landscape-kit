# `lkit self-service`

把 lkit 自身安装为受管服务（lkit 常驻服务化的安装入口）。

```text
lkit self-service install [--service-manager systemd] [--install-dir <PATH>]
lkit self-service remove [--install-dir <PATH>]
```

## install

1. 解析安装根目录（`--install-dir` / `LKIT_INSTALL_DIR` / 默认
   `/root/.lkit/landscape`）并获取安装锁；
2. 选择服务管理器：显式 `--service-manager systemd` 或自动探测；
   `none` 不受支持（无人监管的“服务”没有意义）；
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
- Phase B 只提供最小骨架；事务接管、启动失败看门狗与中断恢复在 Phase C 接入。

## 退出码

- `0`：成功；
- `2`：参数错误（如 `none` 管理器、系统 manager 不可用）；
- `1`：其他失败。
