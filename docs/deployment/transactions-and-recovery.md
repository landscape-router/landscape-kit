# 事务与中断恢复

## 事务文件

### 公共字段与版本

每次首次安装、同版本修复、版本切换、restore 或卸载创建:

```text
/root/.lkit/transactions/<transaction-id>.json
```

事务文件位于 lkit 地盘（见[安装布局与状态](layout-and-state.md)）；卸载清理本根历史，
保留当前卸载的提交记录供 daemon 核实结果。读取支持 schema v1 至 v14，具体操作版本见下文。
事务中的路径字段按基准分为两类：

- 带 `backups/`、`transactions/`、`logs/` 前缀的字段相对 **lkit 地盘**(与事务文件
  同目录体系);
- 带 `releases/`、`data/`、`service/` 或 `current` 前缀的字段相对 **landscape
  安装根**;`install_root` / `canonical_install_root` 字段记录 landscape 根。

示例:

```json
{
  "schema_version": 5,
  "transaction_id": "0198c3d2-0000-7000-8000-000000000001",
  "operation": "switch",
  "phase": "prepared",
  "install_root": "/root/.lkit/landscape",
  "canonical_install_root": "/root/.lkit/landscape",
  "from_version": "0.19.2",
  "target_version": "0.20.0",
  "previous_current": "releases/0.19.2",
  "target_release": "releases/0.20.0",
  "backup": {
    "backup_id": "20260801-163000-a1b2c3d4",
    "path": "backups/20260801-163000-a1b2c3d4.lkb",
    "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
  },
  "restore_backup": null,
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
- `repair_service`；
- `switch`；
- `restore`；
- `uninstall`。

daemon 原生重初始化与迁移的 operation 分别为 `reinit` 和 `migrate`。

修复、切换、恢复和重初始化可记录 `install_workspace`，值必须恰为
`releases/.lkit-transactions/<transaction-id>`，相对该事务的 landscape 安装根。
新事务在修改活动目录前保存该字段，需要原子移动的 data、release 和 static 现场使用
这个同文件系统目录；JSON、状态快照、下载与 `.lkb` 仍放在 lkit 地盘。工作目录必须为
受管根内真实目录，权限 `0700`，拒绝越界、其他事务身份和软链接。缺少该字段的事务
沿用其地盘内现场，恢复时不通过目录存在性猜测另一种布局。

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

`backup` 和 `static_backup` 是所有 schema 的必填可空字段；`restore_backup` 是 schema v4
的必填可空字段，读取 v1/v2/v3 时缺失按 null 处理：

- `switch` 和后端 `repair` 使用 `.lkb` 时，`backup` 必须为对象，`restore_backup` 和 `static_backup` 为 null；
- `lkit repair static` 时，`backup` 和 `restore_backup` 为 null，`static_backup` 必须为对象；
- pending→complete 初始化观测 repair 时，三者均为 null，且不得修改 `current`、版本资产或服务定义；
- 首次 `install` 时三者均为 null；

`restore` 时 `restore_backup` 必须记录用户选择的目标 `.lkb`，`from_version`、`target_version`、
`previous_current` 和 `target_release` 必须同时记录当前与目标版本关系；`backup` 在默认保护备份成功
后记录当前实例的保护 `.lkb`，使用 `--allow-no-backup` 时为 null；`static_backup` 必须为
null。目标备份在进入 `prepared` 前必须已经完整验证并放入 lkit 地盘相对路径，外部路径
不得直接写入事务文件。交互确认（含 minimal scope 数据损失确认）先于 `begin` 完成：
用户拒绝或非交互模式缺少 `--yes` 时不创建事务、不写任何文件，`--file` 也不产生
暂存拷贝。所有新 restore 在目标健康检查后另行等待网络确认；开始恢复前的确认不允许
跳过该窗口，目标 state 仅在网络确认被接受后提交。

`uninstall` 时 `from_version` 和 `previous_current` 必须记录当前已提交版本关系，
`target_version` 和 `target_release` 为 null；`backup` 在默认保护备份成功后记录卸载前
的保护 `.lkb`，使用 `--allow-no-backup` 时为 null，`restore_backup` 和 `static_backup`
必须为 null。卸载确认先于 `begin` 完成：用户拒绝或非交互模式缺少 `--yes` 时不创建
事务、不写任何文件。

`restore_backup.path` 与 `backup.path` 都必须是 lkit 地盘内的安全相对路径；外部备份先复制
到本次 restore 事务目录并完成自校验，事务只记录复制后的路径、backup ID 和文件 checksum。

`no_backup` 是布尔字段，表示用户显式允许缺少保护 `.lkb`，此时 `backup` 必须为 null。
`switch` 仅对已经停止的受管服务允许此例外；`restore` 在保护备份无法创建且用户指定
`--allow-no-backup` 时使用该例外。卸载和 reinit 的显式无备份选项同样记录此字段。
三个服务管理后端共用这些规则；其他事务固定为 false。读取旧 v1 文件时缺失按 false 处理。

`static_backup` Schema 固定为：

```json
{
  "path": "transactions/0198c3d2-0000-7000-8000-000000000001/static-backup",
  "target": "releases/0.19.2/static"
}
```

`path` 相对 lkit 地盘，`target` 相对 landscape 安装根，两个路径都必须是规范路径，
不得逃逸各自基准。备份目录只能包含本次替换前的静态目录内容。

`systemd_before` 和 `resolv_conf_backup` 都是必填但可为 `null` 的字段。需要注册、停止、启动或重启 Landscape 的 systemd 事务，必须在首次修改 systemd 或运行状态前记录 `systemd_before`。需要启动或重启 Landscape 的事务还必须先创建 `resolv_conf_backup`。

`systemd_before` 固定记录事务开始前的服务状态（JSON 键名保持兼容；Rust 侧类型为
`ServiceBefore`,由 [`ServiceManager`](../service/manager.md) 的
`capture_before` 捕获）。systemd 使用带版本的启用快照：

```json
{
  "registration": {
    "kind": "symlink",
    "target": "/srv/landscape/service/landscape-router.service"
  },
  "enabled": {
    "version": 1,
    "enabled": true,
    "systemd_links": {
      "version": 1,
      "boot_id": "11111111-1111-4111-8111-111111111111",
      "persistent": [],
      "runtime": ["multi-user.target.wants"]
    }
  },
  "active": true
}
```

`registration.kind` 只允许 `missing` 或 `symlink`。为 `symlink` 时必须包含绝对字符串
字段 `target`，记录受管定义原件的逻辑路径。systemd 注册可物理指向系统盘的已验证
启动副本，后端将它映射回该原件路径；事务不保存启动副本的物理位置。其他文件类型、
损坏副本或无法证明归属的注册均为冲突，不进入事务。`active` 是事务开始前的运行事实；
启用快照记录持久和运行时成员目录，路径必须是有效的单级 `*.wants`／`*.requires`
名称，不接受越界、重复成员、未知版本或与启用值矛盾的内容。
systemd 的 `linked`、`linked-runtime`、`alias` 不代表
启用，不得据此将原本禁用的服务恢复为启用。

恢复时清理本事务新增的受管成员并重建有效的原成员。运行时成员仅在快照 boot ID
等于当前启动标识时有效；跨整机重启不重新创建，安装 state 的启用字段反映恢复后事实。
`prepared`／`stopping` 阶段的 switch 与 binary repair 恢复，在服务和 resolv 恢复成功后
同步该启用字段，再终结事务；保留原安装版本和提交身份。
不通过普通 `enable` 把运行时成员转换为永久成员。仅理解布尔值的 daemon 无法读取
对象形式的 `enabled`，必须拒绝处理，不得忽略精确信息继续恢复。
OpenRC／SysVinit 及兼容快照可使用布尔 `enabled`；读取这类快照不推测未保存的临时有效期。

恢复注册时，后端从逻辑原件恢复并验证启动副本，保留已有受管启动成员关系，按事务
事实恢复启用／运行状态。原件缺失或注册链接已清理的首次安装失败现场，只能清理仍能证明
属于本原件的副本；不能删除其他安装的副本或覆盖外部链接。服务定义刷新与回滚使用
同一显式同步入口，普通 daemon-reload 不发布未授权的原件修改。

`resolv_conf_backup` 是 lkit 地盘相对路径，固定指向本事务按前述格式创建并自校验成功的 `backups/<transaction-id>/host/resolv.conf` 目录。纯验证、纯静态 repair 不修改运行状态时，该字段为 null。

`network_takeover` 是可空字段，允许出现在首次 `install` 与 `reinit`。它保存用户选择
的接口与 MAC、网络计划、对应后端的宿主网络服务快照、确认截止时间、主机启动标识
及待提交安装状态路径。恢复 unit 名和恢复二进制路径仅用于旧事务兼容清理；新事务
不创建专属恢复 unit。字段不得包含 PPPoE 凭据。

`restore_confirmation` 仅用于 schema v14 的 `restore`，该版本必须提供该对象，旧版本
不得包含它，也不得同时记录 `network_takeover`。它只保存 `boot_id`、
`confirmation_deadline` 和 `pending_state`：主机启动标识及事务 ID 必须为有效 UUID，
待提交状态路径必须恰为 `transactions/<transaction-id>/pending-restore-state.json`，
相对 lkit 地盘。期限在目标启动并通过健康检查后开始；待确认期间保持旧已提交 state。
确认读取 pending state 时校验安装根、本次事务 ID 和目标版本，执行后端必须匹配该状态。
恢复配置不重新接管宿主网络服务，因此不伪造首次接管的网络计划或宿主服务快照。

网络事务的恢复只按[统一恢复契约](../network/daemon-recovery.md)执行：待确认阶段
等待或回滚，`finalizing` 续做提交，`rolling_back` 重试恢复。不能以通用失败清理
替代网络恢复，也不能把已经接受的确认改为回滚。

恢复时按 `systemd_before`（兼容字段名，适用于三个后端）恢复原服务状态，并通过
`resolv_conf_backup` 找到主机备份。缺少必要字段、备份不可用或存在所有权冲突时，
不猜测原状态；网络回滚保留 `rolling_back` 和恢复依据，记录错误供重试或人工修复，
不能标记为终态后放行目标配置启动。其他事务遵守各自的失败恢复规则。

`log_path` 是必填的 lkit 地盘相对路径，固定指向 `logs/<transaction-id>.log`，不得逃逸
lkit 地盘。

事务日志规则：

- 创建事务文件前先创建对应日志文件，所有者 `root:root`、权限 `0600`；
- 记录阶段变化、外部命令结果、已脱敏 URL、HTTP 状态、文件路径、摘要和恢复动作；
- 不记录密码、初始化 TOML 内容、API token、Authorization header、证书私钥或 URL query/fragment；
- 日志写入失败时不得开始或继续修改运行状态；
- 日志随事务保留（位于 lkit 地盘，卸载 landscape 不删除；卸载完成后本根的
  历史事务与日志可以清理，本次卸载提交依据保留，见[卸载语义](../commands/uninstall.md)），失败恢复输出
  必须引用该路径；
- 在 lkit 地盘和事务尚未创建前发生的错误只输出到终端，不另行落盘。

首次安装的 `from_version` 和 `previous_current` 可以为 `null`。事务对象允许未知字段并忽略，以便向后兼容；已定义字段缺失、类型错误或组合不满足上述 operation 规则时，事务损坏。事务不得保存密码、初始化 TOML 内容、API token 或预签名 URL。

Schema v2 相对 v1 新增 `stopping`。Schema v3 新增网络接管字段以及
`awaiting_network_confirmation`、`finalizing`。Schema v4 新增 `restore` operation 和
`restore_backup` 字段。uninstall 使用 v10 记录 `uninstall_keep_data`，该布尔值决定正常清理与中断恢复是否保留 data。读取器兼容 v1/v2/v3；
v1 的 `prepared` 可能来自旧实现中“已经 stop 但尚未写 activating”的窗口，恢复时按可能已经
停止处理。

### 生命周期

- 每次阶段变化都原子更新事务文件；
- 同一台主机只能存在一个未结束事务（lkit 地盘全局唯一）；
- `committed` 和 `rolled_back` 是正常终态；
- `failed` 是异常终态，阻断新事务；
- 历史事务文件保留用于审计；卸载成功可清理本安装根的历史事务与日志，保留本次提交依据（见
  [卸载语义](../commands/uninstall.md)），其他根的历史不受影响；

事务文件无法读取、JSON 无法解析、Schema 不支持、必填字段缺失、阶段非法或路径逃逸
各自基准目录时，必须停止并报告事务状态损坏。不得猜测阶段、重命名损坏文件、创建替代
事务或根据 `current` 自动提交；用户修复或移走损坏事务文件前，不得开始新事务。

### 中断恢复

下次执行发现未结束事务时，先结合 `operation` 和 `phase` 处理：
- `preparing`：尚未改变运行状态；普通事务清理临时文件并标记 `failed`，初始化观测 repair 保持旧状态并标记 `failed`；
- `prepared` 或 `stopping`：`current` 和数据尚未激活，但服务可能已经停止；按
  `systemd_before` 幂等恢复注册链接、enabled/active 状态，清理目标临时资产并标记
  `failed`。
- `uninstall` 的 `preparing`：尚未改变运行状态，清理临时文件并标记 `failed`，用户可
  重新执行卸载；
- `uninstall` 的 `prepared`、`stopping` 或 `activating`：采用**前向完成**语义，不恢复
  systemd 或已提交状态；按 `systemd_before` 查询当前状态后继续完成 stop/disable/
  注销、文件删除并标记 `committed`。恢复再次失败时保留未完成阶段、保护 `.lkb` 与
  事务现场，由 daemon 重试并阻止后续修改；
- `activating` 或 `verifying`：
  - 受管后端下 `no_backup: true` 的 `switch`：停止目标版本，恢复 `previous_current`、
    unit 注册、enabled/active 状态和 `/etc/resolv.conf`，不重建 data；切换前服务已停止，
    因而恢复后保持停止；
  - 受管后端下有 `backup` 的 `switch`：停止目标版本，使用 `previous_current` 和 `.lkb`
    恢复旧版本；
  - 后端 `repair` 且事务含 `.lkb`：按事务记录的受管后端停止修复后的版本，
    使用 `.lkb`、旧二进制和现场快照恢复修复前运行状态；
  - `install`：没有旧版本和 `.lkb`，执行首次安装失败清理；按保存的后端恢复服务注册、宿主网络及 `/etc/resolv.conf`，不得调用 `.lkb` 回滚；
  - `repair` 且 `static_backup` 非空、`backup` 为空：从 `static_backup.path` 恢复 `static_backup.target`，并恢复原版本归档与安装状态，
    不重建 Landscape data；
- `migrate`（手工部署迁移）：安装状态包含相同事务 ID、版本及后端时确认提交；
  准备阶段未停止旧实例时标记失败。停止、激活和验证阶段恢复旧 unit、清理未提交新根，
  成功标记 `rolled_back`；恢复失败保持 `rolling_back`，由 daemon 重试。
  `legacy_unit` 为空的前台进程由用户自行重启，源目录保持不变。
- `restore` 按阶段恢复：
  - `preparing`：尚未改变运行状态，清理事务临时内容并标记 `failed`；
  - schema v14 的 `prepared` 或 `stopping`：进入完整回滚路径，核实原数据并恢复原
    release、current、data、服务及 DNS，成功标记 `rolled_back` 并保留恢复依据；
    旧 v9 按 `systemd_before` 恢复原服务和 DNS，清理事务临时内容并标记 `failed`；
  - `activating`、`verifying` 或 `rolling_back`：停止目标服务，恢复原注册及启用状态但
    不启动；同版本恢复先移回 `replaced-release`，再恢复 `current` 和原 data，最后仅在
    原服务活跃时启动并健康检查，写回原 state 后标记 `rolled_back`。systemd、OpenRC、
    SysVinit 使用同一顺序。必要时使用保护 `.lkb`；恢复失败保留未完成阶段与恢复依据；
  - schema v14 的原 data 由停止前保存的设备号和 inode 识别。`previous-data` 和活动
    `data/` 中只有匹配的真实目录才能视为原数据，不能以目录存在性认定回滚完成。
    原数据丢失时只能使用保护 `.lkb` 重建；无保护备份时阻断恢复，不能放行目标数据；
  - `awaiting_network_confirmation`：同次主机启动且期限未到时等待用户确认，daemon
    重启不延长期限；超时或主机启动标识变化进入回滚；
  - `finalizing`：确认决定已经持久化，校验 pending state 后续做提交，即使期限已到
    或主机已重启也不撤销确认。state 已写但事务未提交时同样续做；
  - 旧 schema v9 不含确认窗口，沿用其停止／激活恢复规则；原 data 已移回后按旧格式
    识别幂等恢复，不补写或猜测 schema v14 的身份依据。
- `rolling_back`：继续事务已经选择的回滚路径，不重新尝试目标版本；
- `awaiting_network_confirmation` 或 `finalizing`：普通命令不执行恢复；用户通过网络
  子命令确认或回滚，daemon 监测待确认期限和主机重启，对 finalizing 继续提交；
- `committed`：不恢复；
- `rolled_back`：不重复恢复；
- `failed`：阻断新安装并要求人工诊断。

普通事务恢复再次失败时标记 `failed`；网络回滚遵守保留 `rolling_back` 的规则。保留可用的 `.lkb`、静态备份、版本目录和失败现场，不无限循环重试。

### 未提交网络接管安装的回滚清理

网络接管首次安装只有在同时满足以下条件时才允许清理整个 `data/`：事务 `operation` 为
`install`，存在 `network_takeover`，阶段为 `awaiting_network_confirmation`
或 `rolling_back`，不存在旧版本、`previous_current`、`.lkb` 或已提交的
`state/install-state.json`，且事务记录的 canonical 安装根目录与当前真实目录一致。

通过校验后，手工 rollback、daemon 超时回滚和确认前主机重启触发的回滚 都必须：

1. 停止并确认未提交的 Landscape 进程退出，再恢复服务注册、enabled/active 状态和 `/etc/resolv.conf`；
2. 恢复事务快照中的宿主网络服务；
3. 删除 `current`、目标 release、临时 current 链接、pending install state 和整个
   `<landscape-root>/data/`；
4. 清理旧事务遗留的 recovery binary、timer 和 service，并将事务标记为 `rolled_back`。

任一恢复或清理步骤失败时不得标记 `rolled_back`，事务保持 `rolling_back`，由 daemon
在后续周期重试。switch、repair 以及已经提交的安装不得使用整棵 `data/` 清理。

## daemon 原生执行

生产环境的 install、reinit、migrate、switch/update、restore、uninstall、repair 和
网络确认／手工回滚由常驻 daemon 接收结构化请求并直接执行。CLI/TUI 负责只读解析、
确认和展示，业务执行及事务恢复不依赖前端进程。daemon 由 systemd、OpenRC 或
SysVinit 托管；迁移旧 systemd unit 的命令要求 systemd。

提交前校验 daemon 存活和对应协议能力。控制台提前展示 daemon 状态并在开始安装前
复查；缺失时提示 `lkit self install`。无需启动同一可执行文件的子进程，因此进程仍在
运行但磁盘上的 lkit 已被升级替换时，已支持的操作仍可执行。

请求保存在 `<territory>/run/operations/`，权限 0600、目录 0700，使用原子写入和磁盘
同步。请求只保存支持的类型与绑定身份；密码使用独立受限文件，领取、取消或恢复时
清理。记录不包含终端、原始 argv、环境变量、工作目录或任意命令执行指令。

daemon 在安装锁下串行调用工作流。操作及事务 ID 的关联在业务事务创建前持久化。
CLI/TUI 观察同一记录；Ctrl+C、SIGHUP 或断线只停止观察，不发送进程组终止信号。
排队请求可以取消；支持暂存下载的操作仅在下载阶段接受取消，切换及恢复阶段拒绝。
`operation status/list/cancel` 不等待安装锁。操作与事务可跨主机启动保留；恢复只
根据持久化证据提交或回滚，不重放无法证明结果的用户请求。

`check`、`reconcile`、`backup`、`self`、`set-mirror`、`software`、`frontend`、
`flare` 与只读操作查询直接执行。自动网络恢复属于 daemon 恢复循环，不再次入队。
`test-support` 的隔离运行时可选择内联测试路径，生产环境没有前端执行回退。

### daemon 自动恢复

安装了 lkit 常驻服务（`lkit self install`）后，daemon 每 2 秒尝试以
非阻塞方式获取安装锁；锁空闲且存在未完成事务时，执行与 CLI 完全相同的本节恢复
语义（见[`lkit self`](../commands/self.md)）：

- CLI 进程因 SSH 断开、崩溃或 `SIGKILL` 消失后，遗留事务由 daemon 自动接管：
  失败激活回滚（含 `.lkb` 配置级回滚）、中断恢复、卸载前向完成均按本节规则执行，
  不再依赖下一次 lkit 调用或委托进程存活；
- daemon 恢复目标固定为 lkit 地盘：从 `/root/.lkit/` 下的状态与事务发现 landscape
  根，不绑定任何具体安装根；
- 网络接管待确认时监测期限与主机启动标识，提交中继续提交，回滚中继续回滚；
  daemon 不代替用户确认，见[网络恢复契约](../network/daemon-recovery.md)；
- 并发安全：业务工作流和直接修改命令持有安装锁，恢复循环获取失败即跳过本周期；
  daemon 收到 `SIGTERM` 时先完成当前周期再退出，不中断进行中的恢复。

`test-support` 运行时可选择 `execution: inline` 或 `daemon`。生产构建不提供该
开关；凡进入本节 daemon 托管边界的命令都固定由 daemon 原生执行。

### 网络恢复元数据

Schema v5 用于 daemon 网络恢复；读取 v1 至 v4 时保留旧事务恢复语义。
`network_takeover.boot_id` 保存接管时的主机启动标识，旧事务缺失时沿用其恢复 unit
并按已有期限处理。OpenRC/SysVinit 的 `host_services[].enable_state` 保存 JSON
形式的后端标识与开机链接快照；systemd 保留原 enable/mask 状态字符串。
操作执行与恢复检查独立调度，修改安装现场仍由安装锁串行化。

## 服务定义修复事务

[`repair service`](../commands/repair.md#服务定义修复) 使用独立可识别的服务定义修复
记录，不根据 `.lkb` 或静态资产备份是否为空猜测修复类型。恢复入口必须能将其与
binary、static 和初始化观测 repair 区分。

替换前持久化原定义字节和权限、系统注册、启用状态、运行状态、安装状态快照及目标
定义身份。事务恢复不调用二进制修复或数据库恢复路径。systemd 的定义修改先原子写入
安装根内原件，再显式同步并校验系统盘启动副本，最后重新加载管理器。原件写入后、副本
同步前的中断由事务决定恢复方向；副本自身的有效身份与摘要不因原件暂时不同而失效。

进入提交阶段前中断恢复原定义和状态，并同步恢复后的副本；服务管理器重新加载失败或
恢复校验失败时保留可重试的非终态。持久化提交阶段后续做目标原件及副本校验和安装
状态提交；失败不得删除原定义备份。原件字节相同但注册布局或副本需要刷新时同样
使用修复事务。操作全过程不重启 Landscape，也不改变持久或运行时的启动成员关系。

服务定义修复的持久化格式必须有显式版本和类型判别。执行前验证运行中的 daemon
支持该格式和对应恢复能力；不支持时在写事务前拒绝，提示先升级并重启 daemon。
未知版本或修复类型不得按普通 repair 清理。失败恢复数据在事务终结前不得删除。

服务定义修复使用 schema v6、`operation: repair_service`；资产修复使用 schema v7、
`operation: repair`；daemon switch 使用 v8，uninstall/reinit/install/migrate 分别使用
v10/v11/v12/v13，restore 使用 v14 并兼容读取 v9。独立快照为 `transactions/<id>/service-repair.json`，记录格式版本 1、后端、
原定义、目标定义、原权限、服务状态、OpenRC／SysVinit 精确启动链接及安装状态。
`preparing` 未修改定义，可标记失败；`prepared`、`activating`、`verifying` 与
`rolling_back` 恢复原定义；`finalizing` 校验并续做提交。恢复遇到外部定义修改或
注册链接冲突时拒绝覆盖并保留事务。运行中的 daemon 通过 `service-repair-v1.pid`
声明支持该恢复类型，标识 PID 必须与实际 daemon 的存活 PID 一致。服务修复提交入口还要求
`operations-v2.pid`。static／binary 修复入口要求 `asset-repair-v2.pid`；
能力标识的 PID 必须与存活 daemon 一致。


## 资产修复事务

static／binary 修复写入 schema v7，操作记录使用 schema v2，事务创建前保存操作与
事务 ID 的关联。binary 事务记录相同的 `from_version` 和 `target_version`；static
通过 `static_backup` 标识备份和目标目录。两者的准备阶段均允许尚未形成完整备份。

static 的 `transactions/<id>/static-snapshot.json` 使用格式版本 1，记录安装状态、
静态目录摘要，以及原 `static.zip` 的存在性、摘要和权限。快照及文件同步完成后才
进入 `prepared`；刷新归档和替换页面均位于 `activating`。恢复先核验快照归属与摘要，
再恢复页面、归档和安装状态，成功标记 `rolled_back`。原归档不存在时恢复为不存在。

binary 备份完成后保存旧状态、服务事实和必要宿主文件；替换失败用 `.lkb` 恢复
数据和原二进制，保持静态目录及归档不变。恢复保留安装记录中的后端和开机启用状态。
恢复失败保持 `rolling_back`，阻断后续修改，保留诊断材料供 daemon 重试。

取消仅在操作层的下载窗口接受；事务处于 `preparing` 并不能单独证明允许取消。
取消清理临时目录并终结事务后，操作记录才写入已清理凭据与 `cancelled`。

### daemon switch 事务

schema v8 专用于 daemon 直接执行的 switch，包括 update 在前端确认后提交的切换。操作 schema v3 记录解析后的目标版本，
事务关联同时核对安装根、from_version 和目标版本。状态快照无论是否创建 `.lkb` 都在
prepared 前持久化；恢复依据是该快照，不能读取可能已经写成目标版本的现场 state。

资产在 `transactions/<id>/target/` 内准备；可信的已有目标 release 先复制进该目录，
自定义前端仅应用到副本。preparing 的取消或失败严格清理事务目录，不删除已有 release。
prepared／stopping 先恢复服务和宿主快照，恢复成功后才终结事务。activating 前关闭取消，
目标 release 在安装文件系统上复制、同步、发布，然后切换 current。已有目标目录在
发布时保留为 `releases/.switch-<id>.previous`；发布临时目录与事务目录用于诊断。

activating／verifying／rolling_back 按旧状态快照回滚；systemd、OpenRC、SysVinit 共用
服务快照恢复。旧版本静态目录和归档保留，`.lkb` 用于恢复二进制及初始化数据。
无备份回滚不恢复 data。恢复失败保持非终态、保留证据并阻止后续修改。

### 原生 restore 事务（Schema v14）

restore 使用 v14，通过操作 ID 关联持久化请求；前端要求 daemon 提供 `restore-v2.pid`
能力标记。目标 `.lkb` 在准备阶段复制到事务目录并记录摘要，执行使用已验证字节。
systemd、OpenRC、SysVinit 依安装 state 的后端执行，恢复时必须与原后端匹配。

`restore_confirmation` 保存当前主机启动标识、截止时间和本事务 pending state 路径。
激活并通过本机健康检查后保存 pending state，进入 `awaiting_network_confirmation`，
原已提交 state 保持不变。所有新 restore 均等待显式 `lkit network confirm`；开始恢复
时的 `--yes`、本机健康检查或 daemon 重启都不能代替用户确认网络可达。期限到达或
确认前整机重启时回滚。确认复核身份、健康和期限后持久化 `finalizing`，之后只续做
目标 state 提交；state 已写而 `committed` 尚未落盘的窗口也不能回滚。

`preparing` 中断时清理临时内容并标记 `failed`；`prepared`、`stopping`、`activating`、
`verifying` 中断都进入完整回滚路径，成功标记 `rolled_back` 并保留恢复依据。

停止服务前，在 lkit 地盘的 `transactions/<id>/previous-data-identity.json` 保存原
`data/` 的设备号和 inode。需要原子移动的 `previous-data`、`failed-data` 和同版本
`replaced-release` 使用事务选定的安装文件系统工作目录。回滚先还原原 release 和
current，再核实并还原原 data；数据已经移回但 daemon 尚未写入下一步时，以目录身份
识别该结果。软链接、身份变化或缺少身份文件不能当作原数据。原 data 丢失时可用
保护 `.lkb` 做配置级重建，无可用依据时保留 `rolling_back` 并阻断启动。

每次回滚重试在修改服务或数据前撤销旧启动放行凭据并同步目录；原 release 与数据
恢复后、启动原服务前写入 `transactions/<id>/restored-boot-id`。启动屏障只在该凭据
等于当前主机启动标识时允许 `rolling_back` 中的原配置启动；它不能放行仍处于待确认阶段的目标配置。原本停止的
服务保持停止。恢复失败保留未完成阶段，daemon 重试并阻止后续修改。

读取旧 v9 restore 时保留其原有恢复语义，不创建或推断确认窗口及数据身份；它不能
使用 v14 的待确认和确认提交阶段。`committed` 只补写操作结果，不重复创建备份或导入
目标。观察者断开不改变任何版本的事务。

### 原生 uninstall 事务（Schema v10）

卸载事务记录 `uninstall_keep_data`，并在创建前关联持久化卸载请求。停止前保存安装
state 快照与服务状态；恢复必须匹配原服务后端。进入 stopping 后按同一事务前向完成，
删除安装状态后仍可从未完成事务发现根。正常提交与重启恢复均保留本次 committed 事务
和日志，操作以此补写成功回执；历史清理不得删除这一提交依据。

### 原生 reinit 事务（Schema v11）

事务固定当前版本，在创建前关联结构化操作，并在修改服务和数据前保存原安装状态。
凭据只用于生成新初始化配置，恢复不重新读取输入文件。网络恢复依据完整时，进入
确认窗口前的中断回滚原数据；待确认阶段保留原期限，接受确认后继续提交。
恢复使用原安装记录的服务后端，后端不匹配时保留现场并拒绝修改。

### 原生 install 事务（Schema v12）

关联操作后写入事务，再在事务目录下载并验证目标资产。下载取消只清理暂存目录，
保留安装根已有的缓存。准备阶段保存 data、service、目标 release 的存在性与完整
快照，并记录摘要、目录权限及服务后端；只有快照完整落盘后才进入 prepared 并修改
安装根、服务和网络。恢复先校验快照与后端身份，失败时保留原事务继续重试。

提交写入的安装状态包含本次事务 ID。状态已落盘但事务尚未标记 committed 时，恢复
必须验证事务 ID、目标版本和服务后端一致才能补全提交；其他安装状态阻断清理。
网络接管沿用显式确认与期限恢复规则，回滚恢复修改前的目录及宿主服务。

## 原生迁移事务

schema v13 的 `migrate` 事务由 daemon 执行。源目录和目标根互不包含；操作请求
绑定源目录设备/inode、进程 PID/starttime、二进制和参数摘要、旧 unit 身份以及版本。
计划确认在提交前完成，daemon 在创建备份和停止旧实例前核对身份。

旧 unit 的 active/enabled、原件路径、备份路径与摘要必须先持久化，再停止、禁用、
移动或 mask。恢复可识别原件尚未移动、备份存在和原件已经恢复的状态，遇到文件冲突
保留现场并等待重试。源进程未退出时不激活新服务。

有效安装状态的 `last_transaction_id`、版本和后端一致时，恢复确认提交；准备阶段
中断不停止旧实例。停止及激活阶段中断回滚新根并恢复旧服务，失败保留 `rolling_back`，
成功标记 `rolled_back`。操作回执依据同一事务收敛，不重新执行迁移。

### 宿主网络恢复快照

网络接管事务的 `preserve_startup_links` 标识 systemd 宿主服务采用保留启动链接的
屏蔽方式；缺省为 false，用于恢复旧事务已经 disable 的服务。
`network_manager` 保存原活动连接的 UUID、接口、物理接口永久 MAC、有效地址族与
默认网关要求，空数组表示没有需要恢复的连接。字段缺失表示旧事务未记录连接身份，
恢复时提示人工核验，不猜测原连接。新事务的连接恢复失败会阻止 `rolled_back`，
保留快照与待提交状态供后续重试。
