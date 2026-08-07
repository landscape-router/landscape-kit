# 安装布局与状态

## 安装根目录

### 选择优先级

安装根目录按以下优先级选择：

1. `--install-dir <path>`；
2. 环境变量 `LKIT_INSTALL_DIR`；
3. 默认值 `/root/.lkit/landscape`。

CLI 参数和环境变量都表示完整安装根目录，不自动追加 `landscape`。

路径必须是绝对路径。每个安装根目录代表一套独立安装；指定另一目录时，不自动寻找、移动或迁移其他目录中的 Landscape。

### 路径与软链接

允许安装根目录或其父目录包含软链接，例如 `/root/.lkit` 指向其他磁盘。

安装器必须同时记录：

- 用户指定或默认得到的 `install_root`；
- 解析后的真实路径 `canonical_install_root`。

规则如下：

- 两个输入路径最终解析到同一真实目录时，视为同一套安装；
- 文件操作和边界判断以真实路径为准；
- 安装过程中真实目标发生变化时立即中止；
- systemd unit 使用真实绝对路径；
- `releases`、`data`、`state`、`transactions`、`backups`、`run` 和 `service` 不得单独成为指向安装根目录外的软链接；
- `current` 是唯一预期的内部受管软链接，并且只能指向本安装根目录内的 `releases/<version>`；
- 安装根目录不能是 `/`、`/root` 或 `/root/.lkit` 等危险父目录。

安装根目录尚不存在时，应解析最近的已存在父目录，再确定最终真实创建位置。

### 目录布局

```text
<install-root>/
├── releases/
│   └── 0.19.2/
│       ├── landscape-webserver
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
├── state/
│   └── install-state.json
├── transactions/
│   └── <transaction-id>.json
├── backups/
│   ├── .tmp/
│   ├── <backup-id>.lkb
│   └── <transaction-id>/
│       └── host/
│           └── resolv.conf/
├── run/
│   └── install.lock
├── logs/
│   └── <transaction-id>.log
└── service/
    └── landscape-router.service
```

职责如下：

- `releases/<version>` 保存该版本后端和该版本当前使用的静态页面；
- `current` 提供原子切换的稳定入口；
- `data` 是 Landscape home path，跨正常版本切换共享；
- 网络接管首次安装在确认前使用的 `data` 属于未提交临时现场；该事务回滚成功时整棵删除，
  不得与已提交安装共享或混淆；
- `state/install-state.json` 只记录最近一次成功提交的安装状态；
- `transactions` 记录进行中和历史事务阶段；
- `backups` 保存 `.lkb` 配置级备份；
- `logs` 保存 lkit 安装事务日志，与 `data/logs` 中的 Landscape 日志分离；
- `service` 保存 `lkit` 生成或接受管理的服务定义原件。

安装根目录及敏感文件由 `root:root` 所有。包含配置、数据库、令牌或备份的文件和目录不得允许非 root 用户读取或写入。

已下载版本和 `.lkb` 备份默认永久保留，v1 不自动清理。

网络接管的未提交首次安装是例外：确认前回滚可以删除本次创建的整个 `data/`，但不得删除
已提交安装、switch、repair 或 service-manager 迁移的数据。

## 并发与原子文件操作

### 安装锁

完成安装根目录规范化和危险路径检查后，`install` 必须在读取安装状态或事务前，对以下文件获取非阻塞独占文件锁：

```text
<install-root>/run/install.lock
```

规则如下：

- 锁使用内核 advisory file lock，例如 Linux `flock(LOCK_EX | LOCK_NB)`；
- 锁已被其他进程持有时立即失败并提示稍后重试，不等待锁释放；
- 锁文件可以永久保留，文件存在本身不表示安装正在运行；
- 安装进程在检查、下载、激活、提交和回滚期间始终保持锁文件描述符打开；
- 进程退出或崩溃后由操作系统释放锁，不通过删除锁文件处理“陈旧锁”；
- 两个输入路径解析到同一 `canonical_install_root` 时必须竞争同一个锁文件。

目标根目录尚不存在或为空时，允许先创建根目录和 `run/` 再获取锁。目标目录包含未知内容时，不得为了加锁而在其中创建文件；应按危险目录规则直接阻断。

### 原子写入

`install-state.json`、事务文件、受管服务定义和 `.lkb` 最终文件必须使用目标同目录或同一文件系统内的临时文件，完整写入并 `fsync` 文件后通过原子 rename 提交。临时文件或 rename 失败时保持原目标不变，不得推进事务阶段。

目标 release 必须在 `releases/` 所在文件系统内的事务临时目录中完整构建并校验，再 rename 为最终 `releases/<version>`。`current` 必须使用安装根目录内的临时软链接加原子 rename 更新，不得先删除旧链接。

遇到跨文件系统 rename 时直接失败，不使用复制后删除的非原子降级。v1 保证进程正常退出或中断时不会提交部分状态文件，但不承诺任意断电点的完整目录级持久化。

## 仓库与资产下载

仓库解析、版本选择、资产下载、校验和静态包解压规范见 [`repository.md`](../repository.md)。

安装流程只消费仓库模块返回的统一发布模型，不直接依赖 GitHub Releases 或第三方静态仓库的元数据格式。

## 安装状态文件

### 职责

`state/install-state.json` 只保存最近一次成功提交的安装状态，不记录正在进行的操作。进行中状态存放在独立事务文件中。

写入必须使用临时文件、`fsync` 和原子替换。只有目标版本通过完整验证后才更新。

### Schema v1

```json
{
  "schema_version": 1,
  "layout_version": 1,
  "install_root": "/root/.lkit/landscape",
  "canonical_install_root": "/root/.lkit/landscape",
  "active_version": "0.19.2",
  "repository": {
    "kind": "github",
    "location": "ThisSeanZhang/landscape"
  },
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

HTTP 仓库来源示例：

```json
{
  "kind": "http",
  "location": "https://repo.example.com/landscape/"
}
```

`location` 不保存预签名 URL，不得包含凭据或敏感 query。

### 字段规则

- `schema_version` 固定为整数 `1`；
- `layout_version` 固定为整数 `1`；
- `active_version` 是规范化 SemVer；
- `repository.kind` 只允许 `github` 或 `http`；
- `assets.webserver` 记录实际落盘后端的架构、大小和可信摘要；
- `assets.static_archive` 只记录安装来源，不用于验证当前静态目录；
- `initialization.status` 只允许 `pending` 或 `complete`；无 systemd 的首次安装尚未由用户启动时为 `pending`；
- `initialization.lock_present` 是提交状态时对初始化锁的观察结果；`status: pending` 时必须为 false，`status: complete` 时必须为 true；
- `initialization.initialized_at` 在 `pending` 时必须为 `null`，在 `complete` 时为 UTC RFC 3339；
- `service.manager` 只允许 `systemd` 或 `none`；
- `registered` 表示是否已向服务管理器注册；
- `enabled` 表示提交时是否设置为开机启动；
- `verified` 表示最近成功事务实际启动并通过健康检查；
- 不记录 `running`，每次执行命令时重新检查实时状态；
- 非空的 `committed_at` 和 `initialized_at` 使用 UTC RFC 3339；
- 未知字段允许并忽略。

早期 schema v1 state 可能包含 `initialization.config_sha256`。读取时把它作为未知兼容字段
忽略，后续写入不再保留；这不会改变 `schema_version`。

### 损坏判定

任一情况使状态文件无效：

- 必填字段缺失或类型错误；
- Schema 或布局版本不支持；
- `repository.kind`、`initialization.status` 或 `service.manager` 使用未定义枚举值；
- SemVer、时间戳、架构或摘要格式非法；
- `canonical_install_root` 与当前真实目录不一致；
- `current` 指向安装根目录之外，或其目标不是 `releases/<active_version>`；
- 状态记录的后端可信摘要、大小或架构字段本身非法；
- `initialization.status: pending` 但 `lock_present != false` 或 `initialized_at != null`；
- `initialization.status: complete` 但 `lock_present != true` 或 `initialized_at` 不是合法 UTC RFC 3339；
- `service.manager: systemd` 但 `registered`、`enabled`、`definition_path` 或 `definition_sha256` 的组合不符合 systemd 状态规则；
- `service.manager: none` 但 `registered != false`、`enabled != false`、`verified != false`，或服务定义路径/摘要不为 null；
- 其他服务字段与 manager 类型或初始化字段与 status 的组合矛盾。

状态文件可解析但后端文件缺失或实际摘要不一致时，属于“受管资产漂移”，不是状态 Schema 损坏，应按 `lkit repair binary` 规则处理。`current` 仍位于安装根目录且仅与 `active_version` 不一致时属于“激活状态漂移”，应阻断并结合事务记录诊断，不得自行选择任一版本。

状态 Schema 损坏时不得根据目录内容猜测重建，不得自行选择当前版本，应停止并给出诊断信息。
