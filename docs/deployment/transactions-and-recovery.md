# 事务与中断恢复

## 事务文件

### Schema v3

每次首次安装、同版本修复、版本切换或 service manager 迁移创建：

```text
<install-root>/transactions/<transaction-id>.json
```

示例：

```json
{
  "schema_version": 3,
  "transaction_id": "0198c3d2-0000-7000-8000-000000000001",
  "operation": "switch",
  "phase": "prepared",
  "install_root": "/root/.lkit/landscape",
  "canonical_install_root": "/root/.lkit/landscape",
  "from_version": "0.19.2",
  "target_version": "0.20.0",
  "from_service_manager": null,
  "target_service_manager": null,
  "previous_current": "releases/0.19.2",
  "target_release": "releases/0.20.0",
  "backup": {
    "backup_id": "20260801-163000-a1b2c3d4",
    "path": "backups/20260801-163000-a1b2c3d4.lkb",
    "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
  },
  "no_backup": false,
  "static_backup": null,
  "systemd_before": {
    "registration": {
      "kind": "symlink",
      "target": "/root/.lkit/landscape/service/landscape-router.service"
    },
    "enabled": true,
    "active": true
  },
  "resolv_conf_backup": "backups/0198c3d2-0000-7000-8000-000000000001/host/resolv.conf",
  "network_takeover": null,
  "log_path": "logs/0198c3d2-0000-7000-8000-000000000001.log",
  "started_at": "2026-08-01T16:20:00Z",
  "updated_at": "2026-08-01T16:30:00Z"
}
```

`operation` 只允许：

- `install`；
- `repair`；
- `switch`；
- `service_migration`。

`phase` 只允许：

- `preparing`；
- `prepared`；
- `stopping`；
- `activating`；
- `verifying`；
- `awaiting_network_confirmation`；
- `finalizing`；
- `rolling_back`；
- `committed`；
- `rolled_back`；
- `failed`。

`backup` 和 `static_backup` 都是必填但可为 `null` 的字段：

- `switch` 和后端 `repair` 使用 `.lkb` 时，`backup` 必须为对象、`static_backup` 为 null；
- `lkit repair static` 时，`backup` 为 null，`static_backup` 必须为对象；
- 无 systemd 的 pending→complete 初始化观测 repair 时，两者均为 null，且不得修改 `current`、版本资产或服务定义；
- 首次 `install` 时两者均为 null；
- `service_migration` 时两者均为 null，且 `from_service_manager` 和 `target_service_manager` 必须分别为不同的 `systemd` 或 `none`。

`no_backup` 是布尔字段，只有用户对已停止的 systemd 服务显式使用
`--allow-no-backup` 的 switch 才为 true；此时 `backup` 必须为 null。其他事务固定为
false。读取旧 v1 文件时缺失该字段按 false 处理。

`from_service_manager` 和 `target_service_manager` 是必填但可为 `null` 的字段。只有 `service_migration` 时两者必须为非 null；其他 operation 必须为 null。迁移事务不得改变 `from_version`、`target_version`、`previous_current` 或 `target_release` 表示的当前版本关系。

`static_backup` Schema 固定为：

```json
{
  "path": "transactions/0198c3d2-0000-7000-8000-000000000001/static-backup",
  "target": "releases/0.19.2/static"
}
```

两个路径都是相对于安装根目录的规范路径，必须留在安装根目录内。备份目录只能包含本次替换前的静态目录内容。

`systemd_before` 和 `resolv_conf_backup` 都是必填但可为 `null` 的字段。普通无 systemd 事务两者必须为 null；需要注册、停止、启动或重启 Landscape 的 systemd 事务，以及任一 `service_migration`，必须在首次修改 systemd 或运行状态前记录 `systemd_before`。需要启动或重启 Landscape 的事务还必须先创建 `resolv_conf_backup`。

`systemd_before` 固定记录事务开始前的服务状态：

```json
{
  "registration": {
    "kind": "missing"
  },
  "enabled": false,
  "active": false
}
```

`registration.kind` 只允许 `missing` 或 `symlink`。为 `symlink` 时必须额外包含绝对字符串字段 `target`，记录 `/etc/systemd/system/landscape-router.service` 的原始链接目标；其他文件类型仍属于所有权冲突，不进入事务。`enabled` 和 `active` 是事务开始前通过 systemd 查询得到的布尔值。

`resolv_conf_backup` 是安装根目录相对路径，固定指向本事务按前述格式创建并自校验成功的 `backups/<transaction-id>/host/resolv.conf` 目录。纯验证、纯静态 repair 和无 systemd 事务不修改运行状态时，该字段为 null。

`network_takeover` 是 v3 新增的可空字段，只允许出现在首次 `install`。它保存用户选择的
接口与 MAC、Landscape 网络计划、NetworkManager/`networking.service`/firewalld/
systemd-resolved 的原始 installed/active/enable 状态、确认截止时间、恢复 unit 名、恢复二进制
和待提交安装状态路径。
字段不得包含 PPPoE 凭据。接管事务在 `awaiting_network_confirmation` 或 `finalizing`
期间不允许通用中断恢复猜测结果，只能执行 `lkit network confirm` 或
`lkit network rollback`。

恢复时必须按 `systemd_before` 恢复注册链接以及 enabled/active 状态，并通过 `resolv_conf_backup` 找到对应主机备份。缺少必要字段、备份不可用或现场出现无法安全覆盖的所有权冲突时，不猜测原状态，事务标记为 `failed` 并要求人工处理。

`log_path` 是必填的安装根目录相对路径，固定指向 `logs/<transaction-id>.log`，不得逃逸安装根目录。

事务日志规则：

- 创建事务文件前先创建对应日志文件，所有者 `root:root`、权限 `0600`；
- 记录阶段变化、外部命令结果、已脱敏 URL、HTTP 状态、文件路径、摘要和恢复动作；
- 不记录密码、初始化 TOML 内容、API token、Authorization header、证书私钥或 URL query/fragment；
- 日志写入失败时不得开始或继续修改运行状态；
- 日志随事务永久保留，失败恢复输出必须引用该路径；
- 在安装根目录和事务尚未创建前发生的错误只输出到终端，不另行落盘。

首次安装的 `from_version` 和 `previous_current` 可以为 `null`。事务对象允许未知字段并忽略，以便向后兼容；已定义字段缺失、类型错误或组合不满足上述 operation 规则时，事务损坏。事务不得保存密码、初始化 TOML 内容、API token 或预签名 URL。

Schema v2 相对 v1 新增 `stopping`。Schema v3 新增网络接管字段以及
`awaiting_network_confirmation`、`finalizing`。读取器兼容 v1/v2；新事务一律写 v3。v1 的
`prepared` 可能来自旧实现中“已经 stop 但尚未写 activating”的窗口，恢复时按可能已经
停止处理。

### 生命周期

- 每次阶段变化都原子更新事务文件；
- 同一安装根目录只能存在一个未结束事务；
- `committed` 和 `rolled_back` 是正常终态；
- `failed` 是异常终态，阻断新事务；
- 历史事务文件保留用于审计。

事务文件无法读取、JSON 无法解析、Schema 不支持、必填字段缺失、阶段非法或路径逃逸安装根目录时，必须停止并报告事务状态损坏。不得猜测阶段、重命名损坏文件、创建替代事务或根据 `current` 自动提交；用户修复或移走损坏事务文件前，不得开始新事务。

### 中断恢复

下次执行发现未结束事务时，先结合 `operation` 和 `phase` 处理：

- `preparing`：尚未改变运行状态；普通事务清理临时文件并标记 `failed`，初始化观测 repair 保持旧状态并标记 `failed`；
- `prepared` 或 `stopping`：`current` 和数据尚未激活，但服务可能已经停止；按
  `systemd_before` 幂等恢复注册链接、enabled/active 状态，清理目标临时资产并标记
  `failed`。无 systemd 事务只清理临时资产；
- `activating` 或 `verifying`：
  - systemd 下 `no_backup: true` 的 `switch`：停止目标版本，恢复 `previous_current`、
    unit 注册、enabled/active 状态和 `/etc/resolv.conf`，不重建 data；切换前服务已停止，
    因而恢复后保持停止；
  - systemd 下有 `backup` 的 `switch`：停止目标版本，使用 `previous_current` 和 `.lkb`
    恢复旧版本；
  - systemd 下的 `repair` 且事务含 `.lkb`：停止修复后的版本，使用 `.lkb` 恢复修复前运行状态；
  - 无 systemd 的 `switch` 或后端 `repair`：只恢复尚未提交的 `current` 或后端文件变更，不启动、不停止、不检查健康，也不执行配置级 data 重建；
  - `install`：没有旧版本和 `.lkb`，执行首次安装失败清理；仅 systemd 环境恢复服务注册和 `/etc/resolv.conf`，不得调用 `.lkb` 回滚；
  - `repair` 且 `static_backup` 非空、`backup` 为空：从 `static_backup.path` 恢复 `static_backup.target`，不重建 Landscape data；
  - `service_migration` 从 `systemd` 到 `none`：按 `systemd_before` 恢复注册链接和 enabled/active 状态，不修改 `current` 或 data；
  - `service_migration` 从 `none` 到 `systemd`：停止本次 systemd 服务，按 `systemd_before` 撤销注册状态并恢复 `/etc/resolv.conf`，保持已提交状态为 `manager: "none"`，不尝试重新启动外部实例；
- `rolling_back`：继续事务已经选择的回滚路径，不重新尝试目标版本；
- `awaiting_network_confirmation` 或 `finalizing`：普通命令不自动处理，要求使用网络
  子命令；超时 timer 或未确认重启执行同一幂等回滚入口；
- `committed`：不恢复；
- `rolled_back`：不重复恢复；
- `failed`：阻断新安装并要求人工诊断。

恢复再次失败时标记 `failed`，保留可用的 `.lkb`、静态备份、版本目录和失败现场，不无限循环重试。

## systemd 托管操作

生产运行时中，任何可能注册、停止、启动或注销 Landscape system unit 的命令，在进入
部署检查和事务前先交给 systemd 托管。lkit 在 `/run/systemd/system` 写入唯一的临时
`lkit-operation-<id>.service`，通过已有 `systemctl` 执行 `daemon-reload` 与
`start --no-block`；不依赖 `systemd-run`。

临时 unit 执行同一 lkit 可执行文件和原始参数。当前工作目录与环境先写入
`/run/lkit/operations/<id>.json`，文件仅 root 可读，worker 读取后立即删除；该文件
可能短暂包含仓库环境凭据，因此不得复制到事务日志或安装根目录。stdout/stderr 写入
同目录临时日志，仍在连接的前端持续转发；结果使用 root-only JSON 原子提交。
`/run/lkit/operations` 固定为 root-only `0700`。下载进度另写入同目录的 root-only
`<id>.presentation.jsonl`，只包含资产显示名称、字节数、耗时和状态，不包含 URL、凭据或
初始化配置。前端 stderr 为终端时使用 Ratatui inline viewport 消费这些事件；非交互前端
消费但不渲染。前端保持连接时，在读取结果、日志和展示事件后删除这些文件；前端已经消失
时，worker 仍删除临时 unit 和请求文件，但结果、stdout/stderr 与展示事件可能保留到主机
重启或管理员手工清理。不得将这些运行时残留描述为已完整自动清理。

Ratatui Install 面板收集的密码不进入原始参数、环境或 request JSON。需要委托时，前端在
同一 root-only operations 目录创建 `<id>.credential`，权限固定为 `0600`，内部子命令只
接收该路径。worker 完成或前端成功停止 operation unit 后删除；停止失败时保留，避免仍在
运行的 worker 读取失败。该文件与其他 `/run` 残留一样最迟在主机重启时消失。

operation unit 固定使用 `StandardInput=null`，不取得 SSH 的 controlling terminal。
前端存在终端时，请求文件只记录其 `/dev` 设备路径；worker 中真正执行命令的子进程以
`O_NOCTTY` 直接打开该设备完成交互。业务命令的退出码写入结果 JSON，wrapper 在结果
落盘后以成功退出，避免可预期的业务失败把已删除的临时 unit 留在 systemd failed set；
只有请求损坏、无法启动子进程或无法写结果等 worker 基础设施错误才令 unit 失败并保留
日志供诊断。

这提供以下边界：

- SSH、终端或调用 lkit 的前端进程消失后，operation unit 与其 cgroup 不受影响，继续
  完成提交或自动回滚；
- 前端收到显式 Ctrl+C 时先恢复原始终端属性和光标，再停止对应 operation unit 及其
  cgroup，清理运行时文件并返回 `130`；停止失败时输出 warning、保留现场并提示 operation
  可能仍在运行；
- 手工 `lkit network rollback` 同样进入 operation unit，避免 NetworkManager 或
  `networking.service` 恢复后当前 `br_lan` SSH 断开而中止回滚；timer/boot 自动回滚已经位于
  独立恢复 unit，不再次委派；
- 交互确认仍通过原终端完成，但 unit 不接管该终端；若终端在破坏性阶段前消失，确认
  读取失败并安全停止；
- systemd worker 不配置自动重试，业务失败不会重复执行整条命令；
- 主机重启会终止 `/run` 中的临时 worker，不承诺跨重启自动继续。下次 lkit 调用按
  本节事务阶段恢复；
- 明确 `service.manager: none` 且不触碰 systemd 的操作保持 inline。

`test-support` 运行时可选择 `execution: inline` 或 `systemd_worker`。生产构建不提供该
开关；凡进入本节 systemd 托管边界的命令都固定使用 worker。
