# 服务、进程与健康检查

## 部署前检查

`install` 必须直接复用 `check` 的结构化检查函数，不执行 `lkit check` 后解析终端文本。

v1 固定检查：

- TCP/UDP `53`；
- TCP `6300`；
- TCP `6443`；
- `check.md` 定义的 root、Linux、内核、BPF、Cgroup、依赖、服务和 DNS 风险；发行版 ID
  只用于诊断，不作为门槛。

结果规则：

- `error`：停止；
- `unknown`：停止；
- `warning`：显示后允许继续；
- `pass`：继续。

`test-support` 的结构化运行时可显式设置 `preflight: skip`，只用于 fake-systemctl
功能测试或真实 systemd 的薄集成测试。该设置不是生产 CLI 参数；生产构建始终执行
本节完整预检，`allow_non_root` 也不再隐式跳过其中若干检查项。

普通安装不自动：

- 卸载或停止 NetworkManager；
- 停止、禁用或 mask `systemd-resolved`；
- 修改 `/etc/network/interfaces`；
- 修改 `/etc/resolv.conf`；
- 调整防火墙、SELinux、sysctl、Cgroup 或内核配置；
- 安装 `iproute2`、`pppd`、Docker 或 Podman。

`lkit install --takeover-network` 是停止、disable 和 mask NetworkManager、Debian
ifupdown 的 `networking.service`、firewalld 与 systemd-resolved 的唯一显式例外。它不卸载
软件包，不修改 SELinux；SELinux 已加载或配置为 enabled/permissive 时在任何变更前拒绝。接管
使用独立的持久回滚机制，见
[网络接管](../network/takeover.md)。

## 进程与旧部署识别

不能依赖 `/proc/<pid>/comm`、`ps` 短名称或 `ss` 展示名称确认 Landscape，因为 Linux 短进程名可能截断，`landscape-webserver` 本身超过常见可见长度限制。

识别流程：

1. 根据固定冲突端口发现候选 PID；
2. 读取 `/proc/<pid>/exe` 的完整执行路径；
3. 通过已打开的 `/proc/<pid>/exe` 文件计算摘要；
4. 读取 NUL 分隔的 `/proc/<pid>/cmdline` 作为辅助证据；
5. 检查参数指向的数据目录和静态目录；
6. 检查候选数据目录是否包含 Landscape 特征文件。

解析 `/proc/net/{tcp,tcp6,udp,udp6}` 时，同一端口可能同时出现监听 socket、健康探测
连接和 TIME_WAIT 条目。实现必须保留该端口的全部非零 inode，再与进程 fd 匹配；不能
用最后一个条目覆盖监听 inode。

判定当前 `lkit` 管理进程必须同时满足：

- 执行文件摘要等于状态记录；
- 执行路径位于真实安装根目录的 `releases/<active-version>`；
- 参数中的数据和静态目录对应当前安装；
- 监听套接字属于该 PID。

即使执行文件已被删除，也应尝试通过 `/proc/<pid>/exe` 打开的文件计算摘要；readlink 结果可能带 ` (deleted)`，不能只比较字符串。

外部 Landscape 需由完整执行路径、参数和 Landscape 特征目录共同确认。仅短名称相同不能确认。

证据不足但固定端口被占用时，报告“无法确认的冲突进程”并阻断。无法读取 `/proc` 或进程在识别中变化时使用 `unknown`，不执行未知二进制的 `--version`。

未占用固定端口、也未引用当前安装目录的同名进程不作为安装冲突。

发现 `/root/.landscape-router` 等旧手工部署数据时，`install` 不迁移它，并拒绝可能覆盖或端口冲突的部署；迁移使用独立的 `lkit migrate` 流程，见
[`lkit migrate`](../commands/migrate.md)。

## 服务管理器集成

`lkit` 通过 [`ServiceManager` trait](manager.md) 操作 init 系统。已实现后端:
`systemd`(默认)、`openrc`、`sysvinit`(简单实现)。后端只改变定义渲染与
生命周期操作的实现,不改变工作流语义。生产运行时按
[探测顺序](manager.md#后端探测顺序)选择可用后端;探测不到任何后端时,
所有需要运行态管理的命令明确失败(退出码 `2`,参数使用错误),不留下任何
安装或事务文件。

### 可用性判断

- systemd 同时满足:`/run/systemd/system` 存在、PID 1 确认为 systemd、
  `systemctl` 存在且可执行、`systemctl show --property=Version` 能连接
  systemd manager;
- OpenRC:`/etc/init.d` 存在、`rc-service`/`rc-update` 可执行且可应答
  (`rc-update --version`),且 PID 1 不是 systemd;
- sysvinit:`/etc/init.d` 存在、`update-rc.d` 可执行,且 PID 1 不是 systemd。

处理规则:

- 全部后端不可用时,`lkit` 不支持在该主机上部署,所有需要运行态管理的命令明确
  失败(退出码 `2`,参数使用错误),不留下任何安装或事务文件;
- 看似使用某 init 系统,但对应工具缺失、无法连接或权限异常:环境损坏,安装失败;
- systemd 为 degraded 不直接阻断,只要 manager 可通信且 Landscape unit 可启动。

`lkit` 明确依赖发行版自启服务,不再提供 `none`(不托管运行态)部署模式。
各后端由 `ServiceManager` trait 定义统一的注册、启停与状态语义,工作流行为不变。

### 命令会话隔离

生产模式下，需要改变 systemd 或 Landscape 运行态的完整 lkit 命令由临时 system
unit 执行。调用进程只负责启动 unit、转发临时日志并等待结果；SSH 会话断开不会向
worker cgroup 传播退出。unit 不取得前端 controlling terminal；需要确认时，worker
子进程直接打开原终端设备。具体文件、结果语义、清理与重启边界见
[事务与中断恢复](../deployment/transactions-and-recovery.md#systemd-托管操作)。

### unit 路径与内容

unit 原件(systemd 后端):

```text
<install-root>/service/landscape-router.service
```

系统注册链接(systemd 后端):

```text
/etc/systemd/system/landscape-router.service
```

OpenRC/sysvinit 后端使用同名定义原件,注册为 `/etc/init.d/landscape-router.service`
软链接,内容为 init 脚本(见 [manager.md](manager.md))。

系统路径必须是指向受管原件的软链接。已存在普通文件、其他目标软链接或无法证明归属时，视为所有权冲突并拒绝覆盖。

unit 至少包含：

- `ExecStart=<canonical-install-root>/current/landscape-webserver --config-dir <canonical-install-root>/data --web <canonical-install-root>/current/static`；
- `User=root`；
- `Restart=always`；
- `LimitMEMLOCK=infinity`；
- `WantedBy=multi-user.target`。

v1 不传端口参数。不得在 `ExecStart`、`Environment=` 或普通环境文件中保存管理员密码。

创建链接后执行 `daemon-reload`、enable 和 start。systemd 集成或启动失败时安装失败并回滚。

### unit 用户变更

状态记录 unit 原件相对路径和 SHA-256。

原件摘要变化时：

- 先解析 unit，确认它仍启动当前安装根目录下的 `current/landscape-webserver`，仍使用当前 `data` 和 `current/static`，并保持 `User=root` 与 `LimitMEMLOCK=infinity`；
- unit 无法解析、改变受管可执行文件或数据目录、加入凭据，或破坏上述安全不变量时阻断，不能通过普通确认接受；
- 仅包含其他可兼容用户修改时，交互模式展示变化并要求确认；
- 非交互模式必须使用 `--accept-service-change`；
- 确认后保留用户内容并更新状态摘要。

系统注册链接不再指向受管原件时属于所有权冲突，普通确认不可绕过。

`lkit` 自身需要升级 unit 格式时，应生成目标内容并展示差异后更新受管原件。

## 不支持的平台

`lkit` 要求主机由受支持的 init 系统管理(见[可用性判断](#可用性判断))。
探测不到任何后端的环境(如容器运行时、无 init 的 chroot)当前**不支持**:

- 探测不到可用服务管理器时,安装、切换、更新、修复、恢复、卸载等命令明确失败并返回
  退出码 `2`(参数使用错误),不创建事务、不写文件;
- 已安装环境(状态记录 `service.manager` 为受支持后端)中对应 init 系统暂时不可用时,
  需要运行态管理的命令同样明确失败,不自动降级为前台进程管理;
- 未来接入其他 init 系统时,由 `ServiceManager` trait 定义统一的注册、启停与状态语义,
  工作流行为不变。

## `/etc/resolv.conf` 主机状态备份

每个需要启动或重启 Landscape 的事务，在对应事务备份目录记录安装前的 `/etc/resolv.conf`：

```text
<install-root>/backups/<transaction-id>/host/resolv.conf/
├── metadata.json
└── content
```

`metadata.json` 示例：

```json
{
  "schema_version": 1,
  "path": "/etc/resolv.conf",
  "file_type": "symlink",
  "symlink_target": "../run/systemd/resolve/stub-resolv.conf",
  "mode": 511,
  "uid": 0,
  "gid": 0,
  "content_saved": false,
  "captured_at": "2026-08-01T16:20:00Z"
}
```

`file_type` 只允许：

- `regular`：内容保存为 `content`；
- `symlink`：只保存链接目标；
- `missing`：表示原本不存在。

目录、设备文件或无法识别类型应在启动 Landscape 前失败。

恢复时：

- 普通文件恢复内容、权限和所有者；
- 软链接恢复原始链接目标；
- 原本不存在时删除本次产生的文件；
- 使用临时文件或临时链接加原子替换。

同版本纯验证且不重启服务时不创建该备份。v1 永久保留，由未来 cleanup 管理。

## 健康检查协议

### 启动等待

本节健康检查只适用于由 `lkit` 通过 systemd 启动的 Landscape。

从 `systemctl start` 成功返回后开始，最长等待 `180` 秒，每秒检查一次。

进程提前退出或 systemd unit 明确进入 `failed` 时立即失败，不等待超时。

全部成功条件：

- TCP `6300` 正常监听；
- TCP `6443` 正常监听；
- UDP `53` 正常监听；当前 Landscape 的普通 DNS listener 不监听 TCP `53`；
- 监听套接字属于目标 Landscape PID；
- `https://127.0.0.1:6443/api/docs` 返回 `2xx` 或 `3xx`；
- HTTPS 检查允许 Landscape 自签名证书。

`/api/docs` 是 v1 固定且稳定的健康检查路径。

首次安装或 `.lkb` 恢复额外要求：

- `landscape_init.lock` 已生成；
- `landscape.toml` 已生成；
- 初始化日志中没有可识别的致命错误。

### 稳定观察

首次达到全部条件后继续观察 `10` 秒：

- PID 不退出或更换；
- systemd unit 不进入 restarting 或 failed；
- 固定端口监听不消失；
- 观察结束时再次请求 `/api/docs` 并获得 `2xx` 或 `3xx`。

任一条件失败即判定目标版本启动失败。
