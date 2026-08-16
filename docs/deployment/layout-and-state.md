# 安装布局与状态

## 双地盘模型

lkit 使用两个分离的目录地盘:

- **lkit 地盘**(固定为 `/root/.lkit/`,不可指定):lkit 自身的元数据与现场——配置、
  安装状态、事务、备份、事务日志与锁。它与 landscape 应用装在哪里无关;
- **landscape 地盘**(`--install-dir`,默认 `/root/.lkit/landscape`):被管理的
  Landscape Router 应用文件。

一台主机只允许一个活跃的 landscape 安装(单实例约束,见[单实例约束](#单实例约束))。
landscape 安装位置记录在 `.lkit/state/install-state.json`,除 `install` 外所有命令
从状态发现 landscape 根,不接收 `--install-dir`。

lkit 自身(CLI 二进制 `/usr/local/bin/lkit` 与常驻 daemon)不属于任何 landscape
地盘,生命周期由 [`lkit self`](../commands/self.md) 管理。

## lkit 地盘(`/root/.lkit/`)

```text
/root/.lkit/
├── config.toml               （可选，用户维护）
├── state/
│   └── install-state.json
├── transactions/
│   ├── <transaction-id>.json
│   └── <restore-transaction-id>/
│       ├── previous-data/
│       └── target-backup.lkb
├── backups/
│   ├── .tmp/
│   ├── <backup-id>.lkb
│   └── <transaction-id>/
│       └── host/
│           └── resolv.conf/
├── logs/
│   └── <transaction-id>.log
└── run/
    ├── install.lock
    └── lkit.pid
```

职责如下:

- `config.toml` 是用户维护的可选配置文件,保存仓库来源偏好;`lkit` 不创建、更新或
  删除它,见[配置文件](config.md);
- `state/install-state.json` 只记录最近一次成功提交的安装状态,并记录 landscape
  安装根目录;
- `transactions` 记录进行中和历史事务阶段;restore 事务还可保留旧 `data/` 和已验证
  的目标备份现场用于中断恢复和人工诊断;
- `backups` 保存 `.lkb` 配置级备份;
- `logs` 保存 lkit 安装事务日志,与 `data/logs` 中的 Landscape 日志分离;
- `run/install.lock` 是安装锁;`run/lkit.pid` 是常驻 daemon 的 pidfile。

lkit 地盘及敏感文件由 `root:root` 所有。包含配置、数据库、令牌或备份的文件和目录
不得允许非 root 用户读取或写入。lkit 地盘不支持软链接指向其他文件系统,也不得是
受管内容的一部分。

## landscape 安装根目录

### 选择优先级

landscape 安装根目录只在 `install`(和 `migrate`)时选择:

1. `--install-dir <path>`;
2. 环境变量 `LKIT_INSTALL_DIR`;
3. 默认值 `/root/.lkit/landscape`。

CLI 参数和环境变量都表示完整 landscape 安装根目录,不自动追加 `landscape`。路径
必须是绝对路径。单实例约束下,`install` 时 `.lkit` 已存在有效安装状态则拒绝,不会
在另一个目录创建第二套安装。

`install` 提交后,landscape 根位置记录在 `state/install-state.json`;`uninstall`、
`update`、`switch`、`restore`、`reinit`、`repair`、`reconcile`、`backup` 等命令从
状态读取根目录,不再接收 `--install-dir`。状态缺失时这些命令按各自规格报错。

### 路径与软链接

允许 landscape 安装根目录或其父目录包含软链接,例如 `/root/.lkit` 指向其他磁盘。

安装器必须同时记录:

- 用户指定或默认得到的 `install_root`;
- 解析后的真实路径 `canonical_install_root`。

规则如下:

- 两个输入路径最终解析到同一真实目录时,视为同一套安装;
- 文件操作和边界判断以真实路径为准;
- 安装过程中真实目标发生变化时立即中止;
- systemd unit 使用真实绝对路径;
- `releases`、`data`、`service` 不得单独成为指向安装根目录外的软链接;
- `current` 是唯一预期的内部受管软链接,并且只能指向本安装根目录内的 `releases/<version>`;
- 安装根目录不能是 `/`、`/root`、`/root/.lkit` 或 lkit 地盘本身等危险父目录。

landscape 安装根目录尚不存在时,应解析最近的已存在父目录,再确定最终真实创建位置。

### 目录布局

```text
<landscape-root>/
├── releases/
│   └── 0.19.2/
│       ├── landscape-webserver
│       ├── static.zip
│       └── static/
├── current -> releases/0.19.2
├── data/
│   ├── landscape_init.toml
│   ├── landscape_init.lock
│   ├── landscape.toml
│   ├── landscape_db.sqlite
│   ├── geo_tmp/
│   ├── logs/
│   ├── metric/
│   └── 其他 Landscape 运行文件
└── service/
    └── landscape-router.service
```

职责如下:

- `releases/<version>` 保存该版本后端、官方静态压缩包和该版本当前使用的静态页面;
  压缩包随该版本保留,供 `.lkb` 备份携带以还原静态资产身份;
- `current` 提供原子切换的稳定入口;
- `data` 是 Landscape home path,跨正常版本切换共享;
- 网络接管首次安装在确认前使用的 `data` 属于未提交临时现场;该事务回滚成功时整棵删除,
  不得与已提交安装共享或混淆;
- `service` 保存 `lkit` 生成或接受管理的 Landscape 服务定义原件。

landscape 地盘不再保存 `state/`、`transactions/`、`backups/`、`logs/`、`run/` 与
`config.toml`——它们全部位于 lkit 地盘(见上)。lkit 常驻服务也不属于 landscape
地盘:lkit 二进制与 `lkit.service` 注册均为全局,见
[`lkit self`](../commands/self.md)。

landscape 地盘及敏感文件由 `root:root` 所有。包含配置、数据库、令牌或备份的文件和
目录不得允许非 root 用户读取或写入。

已下载版本和 `.lkb` 备份默认永久保留,v1 不自动清理。

网络接管的未提交首次安装是例外:确认前回滚可以删除本次创建的整个 `data/`,但不得删除
已提交安装、switch 或 repair 的数据。

### 卸载语义

`lkit uninstall` 是唯一显式清理入口,完整语义见 [`lkit uninstall`](../commands/uninstall.md)。
卸载删除 landscape 根下的全部受管内容(`releases/`、`data/`、`service/` 与 `current`),
并保留 lkit 地盘(`config.toml`、`backups/`、`run/` 与 `transactions/`、`logs/` 目录本身):

- 保护 `.lkb` 存放在 `backups/`,卸载不删除,供用户取走备份;
- 本安装的事务与事务日志在卸载完成后删除(卸载自身的事务记录一并清理):
  卸载后不再有现场价值,新安装不关注上一个安装的残留;只按事务中的
  `canonical_install_root` 清理本根记录,不触碰其他安装根的历史;
- `--keep-data` 额外保留 landscape 根的 `data/`;
- 不存在 `--purge-root`:lkit 地盘不属于 landscape 卸载范围。

卸载成功后 `install-state.json` 不再存在,再次 `lkit install` 按全新首次安装处理。
lkit 常驻服务(若已安装)不受卸载影响,由 `lkit self remove` 单独管理。

### 单实例约束

一台主机只允许一个活跃的 landscape 安装:

- `install` 时 `.lkit/state/install-state.json` 已存在有效状态 → 拒绝(参数错误),
  提示先 `lkit uninstall`;
- 状态只记录最近一次成功提交的安装;卸载或损坏状态按各命令规格处理;
- lkit 常驻 daemon 全局唯一(`lkit.service` 单例),服务对象是固定 lkit 地盘下的
  状态与事务,不绑定某个 landscape 根;重复安装 daemon 前必须先
  `lkit self remove`(见[`lkit self`](../commands/self.md))。

## 并发与原子文件操作

### 安装锁

完成安装根目录规范化和危险路径检查后,`install` 必须在读取安装状态或事务前,对以下
文件获取非阻塞独占文件锁:

```text
/root/.lkit/run/install.lock
```

规则如下:

- 锁使用内核 advisory file lock,例如 Linux `flock(LOCK_EX | LOCK_NB)`;
- 锁已被其他进程持有时立即失败并提示稍后重试,不等待锁释放;
- 锁文件可以永久保留,文件存在本身不表示安装正在运行;
- 安装进程在检查、下载、激活、提交和回滚期间始终保持锁文件描述符打开;
- 进程退出或崩溃后由操作系统释放锁,不通过删除锁文件处理"陈旧锁";
- 锁保护的对象是 lkit 地盘元数据与 landscape 根的全部操作,任何修改状态、事务或
  landscape 根的命令都竞争同一个锁文件。

lkit 地盘尚不存在时,允许先创建地盘和 `run/` 再获取锁。目标目录包含未知内容时,
不得为了加锁而在其中创建文件;应按危险目录规则直接阻断。

### 原子写入

`install-state.json`、事务文件、受管服务定义和 `.lkb` 最终文件必须使用目标同目录或
同一文件系统内的临时文件,完整写入并 `fsync` 文件后通过原子 rename 提交。临时文件或
rename 失败时保持原目标不变,不得推进事务阶段。

目标 release 必须在 `releases/` 所在文件系统内的事务临时目录中完整构建并校验,再
rename 为最终 `releases/<version>`。`current` 必须使用安装根目录内的临时软链接加原子
rename 更新,不得先删除旧链接。

遇到跨文件系统 rename 时直接失败,不使用复制后删除的非原子降级。v1 保证进程正常
退出或中断时不会提交部分状态文件,但不承诺任意断电点的完整目录级持久化。

## 仓库与资产下载

仓库解析、版本选择、资产下载、校验和静态包解压规范见 [`repository.md`](../repository.md)。

安装流程只消费仓库模块返回的统一发布模型,不直接依赖 GitHub Releases 或第三方静态
仓库的元数据格式。

## 安装状态文件

### 位置与职责

`/root/.lkit/state/install-state.json` 只保存最近一次成功提交的安装状态,不记录正在
进行的操作。进行中状态存放在独立事务文件中。它不包含仓库来源信息;分发渠道记录在
独立的用户可编辑 [`config.toml`](config.md) 中。除安装状态外,它还记录 landscape
安装根目录,是所有命令发现 landscape 根的唯一条目。

写入必须使用临时文件、`fsync` 和原子替换。只有目标版本通过完整验证后才更新。

### Schema v1

```json
{
  "schema_version": 1,
  "layout_version": 2,
  "install_root": "/root/.lkit/landscape",
  "canonical_install_root": "/root/.lkit/landscape",
  "active_version": "0.19.2",
  "assets": {
    "webserver": {
      "architecture": "x86_64",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "size": 12345678
    },
    "static_archive": {
      "sha256": "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
      "size": 2345678
    }
  },
  "initialization": {
    "status": "complete",
    "lock_present": true,
    "initialized_at": "2026-08-01T16:30:00Z"
  },
  "service": {
    "manager": "systemd",
    "registered": true,
    "enabled": true,
    "verified": true,
    "definition_path": "service/landscape-router.service",
    "definition_sha256": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
  },
  "last_transaction_id": "0198c3d2-0000-7000-8000-000000000001",
  "committed_at": "2026-08-01T16:30:00Z"
}
```

### 字段规则

- `schema_version` 固定为整数 `1`;
- `layout_version` 固定为整数 `2`(双地盘布局);
- `install_root` / `canonical_install_root` 记录 landscape 安装根目录(不再是 lkit
  地盘);`canonical_install_root` 与当前真实目录不一致时状态损坏;
- `active_version` 是规范化 SemVer;
- `assets.webserver` 记录实际落盘后端的架构、大小和可信摘要;
- `assets.static_archive` 只记录安装来源,不用于验证当前静态目录;
- `initialization.status` 只允许 `pending` 或 `complete`;首次安装尚未由用户启动时为 `pending`;
- `initialization.lock_present` 是提交状态时对初始化锁的观察结果;`status: pending` 时必须为 false,`status: complete` 时必须为 true;
- `initialization.initialized_at` 在 `pending` 时必须为 `null`,在 `complete` 时为 UTC RFC 3339;
- `service.manager` 只允许 `systemd`;
- `registered` 表示是否已向服务管理器注册;
- `enabled` 表示提交时是否设置为开机启动;
- `verified` 表示最近成功事务实际启动并通过健康检查;
- 不记录 `running`,每次执行命令时重新检查实时状态;
- 非空的 `committed_at` 和 `initialized_at` 使用 UTC RFC 3339;
- 未知字段允许并忽略。

早期 schema v1 state(layout_version 1,单根布局)可能包含 `initialization.config_sha256`
或 `repository` 字段。读取时把它作为未知兼容字段忽略,后续写入不再保留;这不会改变
`schema_version`。仓库来源只存在独立的 [`config.toml`](config.md) 中。layout_version 1
的状态在读取时同样忽略 `repository` 且不迁移;旧状态中的路径字段指向旧单一根目录,
按该字段读取 landscape 根。

### 损坏判定

任一情况使状态文件无效:

- 必填字段缺失或类型错误;
- Schema 或布局版本不支持;
- `initialization.status` 或 `service.manager` 使用未定义枚举值;
- SemVer、时间戳、架构或摘要格式非法;
- `canonical_install_root` 与当前真实目录不一致;
- `current` 指向安装根目录之外,或其目标不是 `releases/<active_version>`;
- 状态记录的后端可信摘要、大小或架构字段本身非法;
- `initialization.status: pending` 但 `lock_present != false` 或 `initialized_at != null`;
- `initialization.status: complete` 但 `lock_present != true` 或 `initialized_at` 不是合法 UTC RFC 3339;
- `service.manager: systemd` 但 `registered`、`enabled`、`definition_path` 或 `definition_sha256` 的组合不符合 systemd 状态规则;
- 其他服务字段与 manager 类型或初始化字段与 status 的组合矛盾。

状态文件可解析但后端文件缺失或实际摘要不一致时,属于"受管资产漂移",不是状态 Schema
损坏,应按 `lkit repair binary` 规则处理。`current` 仍位于安装根目录且仅与
`active_version` 不一致时属于"激活状态漂移",应阻断并结合事务记录诊断,不得自行选择
任一版本。

状态 Schema 损坏时不得根据目录内容猜测重建,不得自行选择当前版本,应停止并给出诊断信息。
