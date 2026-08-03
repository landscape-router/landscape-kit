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

首版不自动：

- 卸载或停止 NetworkManager；
- 停止、禁用或 mask `systemd-resolved`；
- 修改 `/etc/network/interfaces`；
- 修改 `/etc/resolv.conf`；
- 调整防火墙、SELinux、sysctl、Cgroup 或内核配置；
- 安装 `iproute2`、`pppd`、Docker 或 Podman。

未来 remediation 流程必须单独设计显式授权、变更预览、备份和恢复规则。

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

发现 `/root/.landscape-router` 等旧手工部署数据时，首版不自动迁移；应拒绝可能覆盖或端口冲突的部署，并提示未来使用独立迁移流程。

## systemd 集成

### 可用性判断

只有同时满足以下条件才视为 systemd 可用：

1. `/run/systemd/system` 存在；
2. PID 1 确认为 systemd；
3. `systemctl` 存在且可执行；
4. `systemctl show --property=Version` 能连接 systemd manager。

处理规则：

- 首次安装显式指定 `--service-manager none`：不执行 systemd 集成，只管理文件和事务；
- 首次安装显式指定 `--service-manager systemd`：全部满足时进行 systemd 集成，否则失败；
- 首次安装未指定 manager 且全部满足：自动选择 systemd；
- 首次安装未指定 manager 且明确不是 systemd init：自动选择 none，只管理文件和事务；
- 看似使用 systemd，但 `systemctl` 缺失、无法连接或权限异常：环境损坏，安装失败；
- systemd 为 degraded 不直接阻断，只要 manager 可通信且 Landscape unit 可启动。

已安装环境未指定 `--service-manager` 时不重新选择 manager，而是继续使用状态文件记录的 `systemd` 或 `none`。显式迁移到 systemd 时必须通过本节全部可用性判断；保持或迁移到 none 时不要求 systemd 可用，但从 systemd 迁移到 none 时必须能连接当前 manager，以安全停止和注销受管服务。

### 命令会话隔离

生产模式下，需要改变 systemd 或 Landscape 运行态的完整 lkit 命令由临时 system
unit 执行。调用进程只负责启动 unit、转发临时日志并等待结果；SSH 会话断开不会向
worker cgroup 传播退出。unit 不取得前端 controlling terminal；需要确认时，worker
子进程直接打开原终端设备。具体文件、结果语义、清理与重启边界见
[事务与中断恢复](../deployment/transactions-and-recovery.md#systemd-托管操作)。

### unit 路径与内容

unit 原件：

```text
<install-root>/service/landscape-router.service
```

系统注册链接：

```text
/etc/systemd/system/landscape-router.service
```

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

## 无 systemd 环境

明确没有 systemd 时，`lkit` 只管理安装文件、备份、软链接、状态和事务，不管理 Landscape 运行态：

- 不启动 Landscape；
- 不停止或向 Landscape 发送信号；
- 不调用 OpenRC、runit、s6、容器运行时或用户脚本；
- 激活后不等待用户启动；
- 不执行端口、PID、`/api/docs`、初始化锁或稳定观察检查；
- 是否启动、何时启动以及如何判断健康完全由用户负责。

首次安装完成后：

- 保留 `data/landscape_init.toml`；
- `landscape_init.lock` 和 `landscape.toml` 可以尚不存在；
- 安装状态记录 `initialization.status: "pending"`、`lock_present: false`、`initialized_at: null`；
- 输出基于真实安装目录的参考启动命令，但不执行该命令：

  ```shell
  '<canonical-install-root>/current/landscape-webserver' --config-dir '<canonical-install-root>/data' --web '<canonical-install-root>/current/static'
  ```

  输出时必须使用适合当前 shell 的安全参数转义；上例单引号仅表示三个路径参数都必须作为独立参数传递。该命令与 systemd unit 的 `ExecStart` 使用相同的可执行文件、data 目录和 static 目录；
- 文件安装和事务提交成功即可返回 `0`，不得宣称 Landscape 已初始化、正在运行或健康。

用户之后再次执行 `lkit install` 时，如果当前状态为 `initialization.status: pending`，且同时检测到普通文件 `landscape_init.lock` 和 `landscape.toml`，应创建一个无 `.lkb`、无 `static_backup` 的轻量 `repair` 事务：

1. 将事务写为 `preparing`；
2. 初始化锁已经出现，说明一次性 init 输入已被消费；不读取或比较现场
   `landscape_init.toml`，文件已被删除也不重建；
3. 重新确认初始化锁和 `landscape.toml` 仍存在且是普通文件；
4. 原子更新状态为 `status: complete`、`lock_present: true`，并把 `initialized_at` 设置为本次首次观察到初始化完成的 UTC 时间；该字段表示 lkit 的首次观察时间，不保证等于 Landscape 实际完成初始化的精确时间；
5. 保持 `service.verified: false`，因为没有执行运行态健康检查；
6. 状态提交成功后将事务直接更新为 `committed`。

该轻量 repair 不启动或停止进程、不访问端口或 API、不创建 `.lkb`，也不改变 active version。任一步失败时保持旧状态并将事务标为 `failed`。

状态中的服务部分记录：

```json
{
  "manager": "none",
  "registered": false,
  "enabled": false,
  "verified": false,
  "definition_path": null,
  "definition_sha256": null
}
```

### 无 systemd 的版本切换与后端 repair

版本切换和后端 repair 仍需要运行中旧版本的导出 API 创建 `.lkb`：

- 当前 Landscape 未运行或导出 API 不可访问时，`lkit` 不代为启动；
- 输出提示，要求用户按自己的方式启动当前版本并重新执行命令；
- 导出和 `.lkb` 创建成功后，`lkit` 要求用户按自己的方式停止当前实例；
- `lkit` 不检查或确认 PID、端口和实际停止状态，只要求用户通过 `/dev/tty` 明确确认已经完成停止；
- 用户确认后才修改 `current` 或后端文件；该确认表示运行态风险由用户承担；
- 该人工检查点要求 `/dev/tty`，v1 不支持无 systemd 环境下无人值守的版本切换或后端 repair；
- 文件激活完成后立即提交状态和事务，不等待用户启动目标版本，也不执行健康检查；
- 提交后的 `service.verified` 为 false。

由于 `lkit` 不观察目标版本启动结果，无 systemd 环境下目标版本之后启动失败不会自动触发回滚。升级前 `.lkb` 仍被保留，用于后续人工恢复能力；本命令只能对激活过程中的文件操作失败恢复旧软链接和文件。

`lkit repair static` 不要求 Landscape 运行或停止，也不做运行态健康检查。

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

本节健康检查只适用于由 `lkit` 通过 systemd 启动的 Landscape。无 systemd 环境不执行本节任何检查。

从 `systemctl start` 成功返回后开始，最长等待 `180` 秒，每秒检查一次。

进程提前退出或 systemd unit 明确进入 `failed` 时立即失败，不等待超时。

全部成功条件：

- TCP `6300` 正常监听；
- TCP `6443` 正常监听；
- TCP 和 UDP `53` 均正常监听；
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
