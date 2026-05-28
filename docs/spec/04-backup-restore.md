# 备份、恢复、升级与回滚

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 术语约定

| 术语 | 含义 |
|---|---|
| `landscape_backup_index.json` | Landscape 维护的权威备份范围定义，管理器只读 |
| `backup.json` | 管理器本地维护的展示索引（非持久化，可重建），用于加速 `backup list` |
| Backup manifest | 备份包内的 metadata JSON（见 4.3），描述当次备份的元信息 |
| Frozen backup index | 备份包内冻结的 backup index 快照，恢复时的权威来源 |
| Release manifest | release 制品的元数据文件（见 03-lifecycle 6.5） |

## 3. 备份点存储与发现

### 3.1 存储路径

- 默认备份目录：`{manager_home}/backup/`（即 `~/.landscape-kit/backup/`）
- 备份文件命名：`lkit-backup-{YYYYMMDD-HHMMSS}-{8位短哈希}.lkb`
- `lkit backup list` 扫描默认备份目录
- `lkit backup restore <id>` 从默认备份目录查找，也支持传入外部文件路径

### 3.2 备份 ID 格式

```
{YYYYMMDD-HHMMSS}-{sha256前8位}
```
示例：`20260528-143022-a1b2c3d4`

## 4. 备份与恢复总体设计

### 4.1 设计目标

备份机制必须同时满足：

- 减轻用户负担
- 提供清晰、稳定的恢复语义
- 优先满足升级前保护与失败回滚
- 避免把"备份什么"变成用户负担

### 4.2 用户心智

V1 不把产品心智设计成"手动选文件备份"，而是设计成"按意图执行动作"。

首版提供两个主动作：

#### A. 导出配置
用途：重新部署、迁移、留档

产物：
- 当前运行态导出的 `landscape_init.toml`
- 简要 manifest

特点：
- 轻量
- 不用于直接恢复已安装实例
- 默认只允许同版本或明确兼容版本使用

#### B. 创建备份点
用途：升级前自动保护、一键回滚、原地恢复

> **权威定义**：以下为默认实例恢复面的唯一权威定义，其他文档引用此处。

默认包含：
- `landscape.toml`
- `landscape_db.sqlite`
- `landscape_init.lock`
- `static/`
- 当次冻结的 `landscape_backup_index.json`
- 备份 manifest

可选包含：
- `metric/`

明确排除：
- `landscape_init.toml`
- `landscape_api_token`
- `logs/`
- `geo_tmp/`

### 4.3 为什么默认备份点包含 `static/`

因为 `static/` 是前端运行所需的发布资源。默认恢复成功的定义应包含：

- Landscape 后端可启动
- Web UI 可访问

因此 `static/` 不能按缓存或临时目录处理，而应纳入默认实例恢复面。

### 4.4 为什么默认备份点不包含 `landscape_init.toml`

因为它不是已安装实例的主恢复面。已安装实例的主要恢复对象是：

- `landscape.toml`
- `landscape_db.sqlite`
- `landscape_init.lock`
- `static/`

`landscape_init.toml` 更适合作为导出配置产物，用于重建与迁移。

### 4.5 备份保留策略

| 类型 | 策略 |
|---|---|
| 自动备份（升级前创建） | 保留最近 5 个，超过时删除最旧的 |
| 手动备份 | 永久保留，由用户手动删除 |
| 空间不足 | 拒绝创建新备份，提示用户清理 |

## 5. Landscape 提供的备份索引文件

### 5.1 设计目标

建议向 Landscape 提交 PR，使其在 HOME 下维护：

- `${LANDSCAPE_HOME}/landscape_backup_index.json`

该文件由 Landscape 维护，管理器只负责读取与执行。

### 5.2 设计原则

这个文件不是简单文件列表，而是：

- 备份源定义
- 恢复策略声明
- 版本兼容范围声明

### 5.3 V1 约束

- 路径只能是 **HOME 相对路径**
- 不允许绝对路径
- 不允许 `..`
- 不允许通过软链逃逸 HOME 根目录
- 管理器创建备份时，必须将当次读取到的 index 原样封入备份包
- 恢复时优先使用备份包内冻结的 index，不依赖 live 系统上的当前 index

### 5.4 推荐结构

```json
{
  "schema_version": 1,
  "landscape_version": "0.18.2",
  "generated_at": "2026-04-18T20:30:00Z",
  "base_dir": ".",
  "profiles": {
    "default": [
      "runtime_config",
      "init_lock",
      "database",
      "web_assets"
    ],
    "with_metrics": [
      "runtime_config",
      "init_lock",
      "database",
      "web_assets",
      "metric_dir"
    ]
  },
  "entries": [
    {
      "id": "runtime_config",
      "path": "landscape.toml",
      "kind": "file",
      "backup_policy": "if_exists",
      "restore_policy": "replace",
      "presence_tracking": "exact",
      "default_selected": true,
      "restore_version_scope": "exact",
      "category": "config"
    },
    {
      "id": "init_lock",
      "path": "landscape_init.lock",
      "kind": "file",
      "backup_policy": "if_exists",
      "restore_policy": "replace",
      "presence_tracking": "exact",
      "default_selected": true,
      "restore_version_scope": "exact",
      "category": "init"
    },
    {
      "id": "database",
      "path": "landscape_db.sqlite",
      "kind": "file",
      "backup_policy": "if_exists",
      "restore_policy": "replace",
      "presence_tracking": "exact",
      "default_selected": true,
      "restore_version_scope": "exact",
      "category": "data"
    },
    {
      "id": "web_assets",
      "path": "static",
      "kind": "dir",
      "backup_policy": "if_exists",
      "restore_policy": "replace",
      "presence_tracking": "exact",
      "default_selected": true,
      "restore_version_scope": "exact",
      "category": "web_assets"
    },
    {
      "id": "metric_dir",
      "path": "metric",
      "kind": "dir",
      "backup_policy": "if_exists",
      "restore_policy": "manual",
      "presence_tracking": "content_only",
      "default_selected": false,
      "restore_version_scope": "exact",
      "category": "metrics"
    }
  ]
}
```

### 5.5 字段语义

- `backup_policy: if_exists`
  - 存在则备份，不存在不报错
- `restore_policy: replace`
  - 恢复时覆盖目标
- `restore_policy: manual`
  - 默认不自动恢复，需要用户主动勾选
- `presence_tracking: exact`
  - 恢复时要把"存在/不存在"的状态也恢复一致
- `presence_tracking: content_only`
  - 只恢复备份中实际存在的内容，不负责清理目标
- `restore_version_scope: exact`
  - V1 只允许同版本恢复

## 6. 备份包文件格式

### 6.1 设计目标

V1 备份格式应具备：

- 可快速识别
- 可校验
- 可扩展
- 能独立自描述

### 6.2 推荐格式

采用自定义容器格式：

1. 固定 magic header
2. format version
3. metadata length
4. metadata JSON
5. 压缩归档 payload

### 6.3 metadata 最少包含

- backup id（格式见 3.2）
- created_at
- sequence
- hostname / target id
- landscape version
- manager version
- source home path
- frozen backup index
- payload format
- compression format
- sha256 checksum
- remark

### 6.4 压缩与校验

- V1 使用 `tar.gz` 作为 payload
- 统一使用 `sha256`
- 不使用 `md5`

### 6.5 staging 模式与原子性

V1 采用 **staging tmp** 模式：

1. 先做 precheck
2. 停止 Landscape 服务（确保数据库一致性）
3. 将需备份内容复制到 staging 目录
4. 再统一打包压缩
5. 启动 Landscape 服务
6. 校验归档可读后，使用 `write-tmp + fsync + rename` 模式原子写入 backup 目录（文件级原子）

恢复流程的原子性：

- 先写入 staging 目录，再逐项替换目标文件
- 采用 all-or-nothing 语义：如果中途失败，保留原始文件不变，不产生半恢复状态
- 多文件恢复无法做到真正的文件系统级原子操作，但保证失败后可回退

### 6.6 空间检查

不写死"必须预留 3 倍空间"，而是：

- 根据备份模式估算所需空间
- 至少覆盖：staging 数据量 + 压缩产物 + 安全余量
- 空间不足则拒绝执行备份

## 7. 恢复模型

### 7.1 恢复类型

恢复必须拆成两类：

#### A. 实例恢复（默认恢复）

适用于：
- 升级失败
- 配置/数据库损坏
- 原地回退到某个备份点

恢复对象：
- `landscape.toml`
- `landscape_db.sqlite`
- `landscape_init.lock`
- `static/`
- 可选 `metric/`

#### B. 按配置重建

适用于：
- 新机器重部署
- 同版本重建
- 从当前运行态导出的 init 配置再次部署

输入：
- 导出的 `landscape_init.toml`

### 7.2 实例恢复流程

1. 读取备份 metadata 与 frozen backup index
2. 校验版本兼容性（V1 默认 exact）
3. 停止 Landscape 服务
4. 还原 required 项到 staging 区
5. 逐项替换目标文件（all-or-nothing 语义）
6. 启动 Landscape
7. 进行 health check
8. 成功则记录恢复结果，失败则进入失败处理

### 7.3 配置重建流程

1. 将导出的 `landscape_init.toml` 放入目标 HOME
2. 根据重建模式处理 `landscape_init.lock`
3. 首次启动 Landscape
4. 触发初始化逻辑
5. 初始化结果写入 `landscape.toml` 与 `landscape_db.sqlite`

### 7.4 恢复安全约束

- 默认不做跨版本 init 配置恢复
- 默认不恢复被排除项
- 恢复前必须做兼容性检查
- 恢复后必须做健康检查

## 8. 升级与回滚设计

### 8.1 总体原则

升级与回滚分两层：

1. **版本回滚**：回退程序版本 / 包版本
2. **实例回滚**：回退配置与数据库状态

### 8.2 升级流程

1. 检查目标版本与 release source
2. 创建自动备份点
3. 获取 release 产物（通过多源并发探测，详见 [09-release-source.md](./09-release-source.md)）
4. 执行升级（V1 以 systemd 托管的二进制替换/部署为主）
5. 启动 Landscape
6. 进行 health check
7. 失败则进入回滚流程

### 8.3 更新事务语义

V1 中，更新不是单纯的"下载并替换二进制"，而是受控事务：

1. 检查可升级版本
2. 创建自动备份点
3. 通过多源并发探测选择最优源，下载并校验 binary 与 `static.zip`（详见 [09-release-source.md](./09-release-source.md)）
4. 停止 Landscape 服务
5. 应用新版本
6. 启动服务
7. 校验 API 与 Web UI 静态资源可访问性
8. 任一步失败则进入回滚

### 8.4 回滚流程

#### A. 版本回滚
- 切回旧版本二进制 / release 版本（回滚 ID 即升级前自动备份的备份点 ID）

#### B. 实例回滚
- 恢复升级前创建的备份点
- 默认恢复面包括：
  - `landscape.toml`
  - `landscape_db.sqlite`
  - `landscape_init.lock`
  - `static/`
  - 可选 `metric/`

### 8.5 核心要求

- 升级前必须创建回滚点
- 回滚后必须做健康检查
- 回滚点必须有版本信息与可追踪 ID
- 自动备份与手动备份应区分保留策略
- 更新失败时不能只回退二进制，必须同时考虑实例状态回退
- CLI 中必须提供手动备份、升级、回滚的直接入口
