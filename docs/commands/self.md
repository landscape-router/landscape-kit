# `lkit self`

管理 lkit 自身:安装/升级/移除全局常驻 daemon。lkit CLI 二进制(`/usr/local/bin/lkit`)
由 `install.sh` 安装,不属于本命令管理范围;`self upgrade` 会更新它。

lkit 自身与 landscape 安装完全解耦:daemon 不绑定任何 landscape 根,恢复目标固定为
lkit 地盘(`/root/.lkit/`),见[安装布局与状态](../deployment/layout-and-state.md)。
因此 `self remove` 不要求 landscape 已卸载,`uninstall` 也不影响 daemon。

```text
lkit self install
lkit self upgrade [--version <TAG>]
lkit self remove
```

`self` 命令都不接收 `--install-dir`。

daemon 全局唯一(`lkit.service` 单例)。daemon 进程写 pidfile 到
`/root/.lkit/run/lkit.pid`(`0600`,原子替换);同地盘已存在存活实例时拒绝启动。

## install

把 lkit 注册为全局常驻服务:

1. 获取安装锁;
2. 服务管理器固定为 systemd;
3. 校验 `/usr/local/bin/lkit` 可执行,并把 unit 定义原件渲染到全局目录
   `/usr/local/lib/lkit/lkit.service`:`ExecStart=/usr/local/bin/lkit daemon`,
   `User=root`、`Restart=always`、`WantedBy=multi-user.target`;
4. 注册(注册链接 `/etc/systemd/system/lkit.service` → 全局原件)、启用并启动服务,
   校验 MainPID 非零。

重复执行时:若旧 daemon 仍在运行,注册完成后执行 `restart` 使其加载当前二进制;
注册或启动失败时尽力回滚已注册状态并删除定义原件。`install` 不复制二进制——daemon
直接使用 `/usr/local/bin/lkit`,升级由 `self upgrade` 负责。

## upgrade

把 lkit CLI 与 daemon 一起升级到目标版本(默认最新 stable):

1. 获取安装锁;
2. 解析目标版本:默认 GitHub `releases/latest` 的 stable;`--version <TAG>` 指定
   版本(候选版必须用带 tag 的版本,例如 `v0.2.0-rc.1`);
3. 下载对应架构二进制(`lkit-x86_64` / `lkit-aarch64`)与 `SHA256SUMS`,校验 SHA-256,
   与 `install.sh` 同源同校验规则(见[自发布](../release/lkit.md));
4. 版本对比:当前版本与目标版本相同 → 输出提示并返回 `0`,不修改任何文件;
5. 下载并校验成功后,对替换后的二进制执行 `lkit --version` 自检,再原子替换
   `/usr/local/bin/lkit`;下载、校验、自检或替换失败时保留原二进制;
6. 刷新 daemon:若 `lkit.service` 已注册且 `is-active` → `restart` 加载新二进制;
   已注册但未运行 → 不启动;未注册 → 仅更新 CLI,并提示可用 `lkit self install`
   安装 daemon。

`upgrade` 不创建事务、不创建保护备份;daemon 在 `restart` 期间短暂退出,由
`Restart=always` 恢复。替换运行中的 daemon 二进制是安全的:进程继续使用已打开的
旧 inode,重启后加载新文件。

## remove

停止并注销 lkit daemon,幂等可重复:

1. 获取安装锁;
2. 停止并等待 lkit daemon 退出;
3. disable、注销注册链接并执行刷新;
4. 删除 `/usr/local/lib/lkit/lkit.service` 原件(空目录一并移除)。

`remove` 不删除 `/usr/local/bin/lkit`(CLI 由安装脚本管理),不触碰任何 landscape
安装根与 lkit 地盘元数据(`config.toml`、`backups/`、`transactions/` 保留)。

## daemon 进程

`lkit daemon` 是常驻服务本体,固定读取 lkit 地盘 `/root/.lkit/`:

- pidfile 写入 `/root/.lkit/run/lkit.pid`(`0600`,原子替换);已存在存活实例时拒绝启动;
- 收到 `SIGTERM` / `SIGINT` 后清理 pidfile 并退出;
- **周期中断恢复**:每 2 秒尝试以非阻塞方式获取安装锁,锁空闲且存在未完成
  事务时,执行与 CLI 相同的 `recover_interrupted` 语义——CLI 因 SSH 断开或
  崩溃消失后,遗留事务由 daemon 自动接管(失败激活回滚、中断恢复、卸载前向
  完成等,详见[事务与中断恢复](../deployment/transactions-and-recovery.md));
  恢复目标从 lkit 地盘的状态与事务发现 landscape 根;
- 网络接管待确认阶段仍由 `lkit network confirm|rollback` 人工处理,daemon 不代替确认;
- CLI 命令持有安装锁期间 daemon 自动让行,不产生并发冲突。

## 退出码

- `0`:成功;
- `2`:参数错误(如请求 systemd 但不可用、`--version` 非法);
- `1`:其他失败。

`upgrade` 版本相同或已是最新时返回 `0`。
