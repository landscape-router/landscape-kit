# 备份、恢复、升级与回滚

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 术语约定

| 术语 | 含义 |
|---|---|
| `metadata.json` | 备份包内的元信息 JSON，描述当次备份的元信息 |
| 离线恢复包 | 备份的产物，包含 init 配置 + 二进制 + 前端资源，可在无网络时重建 |
| Release manifest | release 制品的元数据文件 |

## 3. 备份点存储与发现

### 3.1 存储路径

- 默认备份目录：`{manager_home}/backup/`（即 `~/.landscape-kit/backup/`）
- 备份文件命名：`lkit-backup-{YYYYMMDD-HHMMSS}-{sha256[:8]}.tar.gz`
  - 创建流程：先写临时文件 → 计算完整文件 sha256 → 取前 8 位 → 重命名
  - 避免 chicken-and-egg：文件名中的 hash 是对完整 tar.gz 文件的 hash
- `lkit backup list` 扫描默认备份目录
- `lkit backup restore <id|path>` 自动识别：匹配 `{YYYYMMDD-HHMMSS}-{sha256[:8]}` 格式的视为 ID 在备份目录查找，否则视为直接文件路径

### 3.2 备份 ID 格式

```
{YYYYMMDD-HHMMSS}-{sha256前8位}
```

示例：`20260528-143022-a1b2c3d4`

### 3.3 备份目录不存在时

首次使用自动创建 `{manager_home}/backup/` 目录。

## 4. 备份与恢复总体设计

### 4.1 设计目标

- 提供离线恢复能力：无需网络即可重建 Landscape 实例
- 轻量：只备份必要文件，不包含数据库快照
- 原子性：备份包写入和恢复过程保证 all-or-nothing
- 可自描述：单文件独立，不依赖外部索引

### 4.2 备份内容

#### 默认包含

- `landscape-webserver` — Landscape 可执行二进制
- `static/` — 前端静态资源（含 index.html 和 JS/CSS 资产）
- `landscape_init.toml` — 当前运行态配置（通过 API `GET /api/v1/system/config/export` 导出）

#### 逻辑说明

`landscape_init.toml` 包含所有核心配置（网卡、路由、DNS、防火墙、DHCP 等），
因此无需额外备份数据库。恢复时相当于用导出的配置在新位置重新初始化。

不备份数据库 (`landscape_db.sqlite`) — 恢复时从 init 配置重建，保证配置一致性。

#### 明确排除

- `landscape_api_token` — 令牌不可移植
- `logs/`、`geo_tmp/`、`metric/` — 运行时临时数据，与配置无关
- `landscape_backup_index.json` — 不存在，无需依赖

### 4.3 备份文件格式

采用标准 `tar.gz` 格式，无自定义容器协议。用户可用 `tar xf` 直接查看内容。

```
lkit-backup-{YYYYMMDD-HHMMSS}-{sha256[:8]}.tar.gz
├── metadata.json
├── landscape-webserver
├── landscape_init.toml
└── static/
    ├── index.html
    ├── assets/
    └── scalar/
```

### 4.4 metadata.json 结构

```json
{
  "format_version": 1,
  "backup_id": "20260531-143022-a1b2c3d4",
  "created_at": "2026-05-31T14:30:22Z",
  "landscape_version": "0.19.2",
  "hostname": "build2026",
  "checksums": {
    "landscape-webserver": "sha256hex",
    "landscape_init.toml": "sha256hex"
  },
  "remark": "升级前自动备份",
  "auto": true
}
```

### 4.5 二进制发现策略

备份时需要定位运行中的 `landscape-webserver` 二进制路径：

1. **优先**：遍历 `/proc/*/exe`，匹配进程 comm 名包含 `landscape-webserver` 的进程，取其二进制路径
   - 如果找到多个匹配进程，取第一个
   - 如果没有找到任何匹配进程，报错并提示用户先启动 Landscape 服务
2. **回退**：`{LANDSCAPE_HOME}/landscape-webserver`（安装时的约定路径），仅当 /proc 发现失败且 API 不可达时

备份时将二进制复制到 staging，不直接引用原路径。

### 4.6 压缩与校验

- 使用 `tar.gz`（gzip 压缩）
- 统一使用 `sha256` 校验
- 备份文件创建时设 0600 权限（内含 TLS 私钥等敏感信息）
- 不提供分割/加密（V1 不涉及）

### 4.7 备份流程（无需停服务）

由于备份期间 binary 和 static 在运行时只读（仅安装/升级时变更），
备份全程无需停止 Landscape 服务：

1. 创建备份目录（`create_dir_all`），然后执行空间检查（§4.9 备份策略）
2. 调用 API `export_config()` 获取 `landscape_init.toml` 内容
3. 通过 `/proc/*/exe` 发现运行中的 binary 路径，复制到 staging
4. 从 `LANDSCAPE_HOME` 复制 `static/` 到 staging
5. 写入 `metadata.json`
6. 打包为 `tar.gz`（staging 目录 → 归档，设 0600 权限）
7. 校验归档可读后，计算 sha256，使用 `write-tmp + fsync + rename` 模式原子写入 backup 目录
8. 清理 staging 目录

### 4.8 恢复流程

#### 4.8.1 SSH 断连保护设计

恢复过程涉及停服务→替换文件→启服务，期间若 SSH 断开则进程可能被终止。
采用 systemd-run 将实际恢复逻辑作为 systemd transient service 运行，
确保 SSH 断开后恢复继续执行，不受终端会话生命周期影响。

#### 4.8.2 双层保护机制

| 层级 | 机制 | 触发条件 |
|---|---|---|
| systemd-run | 进程托管给 systemd，脱离 SSH 会话 | 始终启用 |
| 恢复前快照 | 替换前复制当前文件到 recovery 目录 | 始终启用 |
| auto rollback | health check 失败时从 recovery 恢复 | health check 不通过 |

#### 4.8.3 restore 主命令（前台）

`lkit backup restore <id|path>`：

1. **参数识别**：参数匹配 `{YYYYMMDD-HHMMSS}-{sha256[:8]}` 格式时，从 `{manager_home}/backup/` 拼接文件名查找；否则视为直接文件路径
2. 校验备份包完整性（checksum、格式）
3. 确认后提示用户"将通过 systemd 在后台执行恢复"
4. **创建当前状态快照**：复制当前 `landscape-webserver` + `static/` + `landscape_init.toml`（如存在）到 `{manager_home}/runtime/recovery-{timestamp}/`
5. 通过 `systemd-run --unit=lkit-restore --same-dir --collect` 启动后台 service 执行 `_do_restore`
6. 提示用户：恢复已启动，查看进度：`journalctl -u lkit-restore -f`

#### 4.8.4 _do_restore 隐藏子命令（后台 service 执行）

`lkit backup _do_restore <id> <recovery_path>`：

此命令标记为 `#[command(hide = true)]`，不对外展示，仅由 systemd-run 内部调用。

1. 停止 Landscape 服务
2. 解压备份包到 staging 目录
3. 替换 `{LANDSCAPE_HOME}/landscape-webserver`（保持执行权限 0755）
4. 替换 `{LANDSCAPE_HOME}/static/`
5. 写入 `{LANDSCAPE_HOME}/landscape_init.toml`
6. 启动 Landscape 服务
7. 执行 health check（端口可达性检查，不依赖 API）
8. **health check 通过**：删除 recovery 目录 → 输出成功到 journal → 退出码 0
9. **health check 失败**：从 recovery 目录恢复原文件 → 启动服务 → 输出错误到 journal → 退出码 1

### 4.9 空间检查

备份和恢复采用不同的空间检查策略：

| 操作 | 计算公式 | 说明 |
|---|---|---|
| 备份 | `max(20 MiB, need_bytes × 20%)` | `need_bytes` 为 staging 数据量（binary + static + text）的估算值；最低保障 20 MiB，防止小备份时余量过低 |
| 恢复 | `(file_size - 1 MiB_header) × 5` | `file_size` 为备份包总大小；减去约 1 MiB 的 metadata/header 后乘以 5，作为解压与替换所需空间的上界 |

- 空间不足则拒绝执行对应操作

### 4.10 保留策略

| 类型 | 策略 |
|---|---|
| 自动备份（升级前创建） | 保留最近 5 个，超过时删除最旧的 |
| 手动备份 | 永久保留，由用户手动删除 |
| 空间不足 | 拒绝创建新备份，提示用户清理 |

### 4.11 自动备份清理

- `lkit upgrade` 执行升级时，创建自动备份后，自动清理超出 5 个的旧自动备份
- 清理规则：只删除 `auto: true` 的备份，按 `created_at` 升序删除最旧的
- 不提供独立的 `lkit backup prune` 命令（V1）

## 5. CLI 命令结构

### 5.1 子命令

```
lkit backup create [--remark <text>]            创建备份点
lkit backup list [--json]                       列出现有备份
lkit backup restore <id|path>                   从备份点恢复（systemd 后台执行）
lkit backup rebuild <id> --target <path>        解压到指定路径（离线重建，不启服务）
lkit backup delete <id>                         删除备份点
lkit backup _do_restore <id> <recovery_path>    (隐藏) 实际恢复逻辑，由 systemd-run 调用
```

### 5.2 创建备份

1. 验证 Landscape 已安装且进程运行中
2. 发现二进制路径（/proc 扫描）
3. 空间预检
4. 调 API export_config → staging binary + static → 打包 → 写入备份目录

### 5.3 列表展示

扫描 `{manager_home}/backup/*.tar.gz`：
- 通过 `gzip -dc | tar -xO metadata.json` 提取元信息（无需完全解压）
- 按创建时间降序排序
- 展示列：ID、创建时间、Landscape 版本、自动/手动、备注
- `--json` 参数输出 JSON 格式

### 5.4 恢复

见 §4.8 恢复流程。恢复主命令只做 precheck + 创建 recovery 快照 + 启动 systemd service。实际替换逻辑在隐藏子命令 `_do_restore` 中。

### 5.5 重建

`lkit backup rebuild <id> --target <path>`：

1. 校验备份包完整性
2. 解压到目标路径（不检查版本、不操作服务）
3. 保持 binary 执行权限
4. 输出："已解压到 {path}"

### 5.6 删除

- 校验备份 ID 存在
- 自动备份可删除
- 手动备份删除时额外提示确认

### 5.7 交互式菜单

- `menu.backup` 从 `NotImplemented("M3")` → Dispatch 到 `lkit backup create`
- `menu.restore` 从 `NotImplemented("M3")` → 触发交互式 restore 选择

## 6. 升级与回滚设计

### 6.1 总体原则

升级与回滚分两层：

1. **版本回滚**：回退二进制版本
2. **实例回滚**：通过备份点恢复 init 配置 + 前端资源

### 6.2 升级流程

1. 检查目标版本与 release source
2. **创建自动备份点**（调用 `backup create`，auto=true，remark="升级前自动备份"）
3. 自动清理旧的自动备份（保留最近 5 个）
4. 获取 release 产物（通过多源并发探测）
5. 停止 Landscape 服务
6. 替换 binary + static
7. 启动 Landscape
8. 进行 health check
9. 失败则进入回滚流程

### 6.3 回滚流程

回滚即从备份点恢复（见 §4.8 恢复流程）。

### 6.4 核心要求

- 升级前必须创建回滚点
- 回滚后必须做健康检查
- 自动备份与手动备份应区分保留策略
- CLI 中必须提供手动备份、升级、回滚的直接入口

## 7. 安全约束

- 恢复前自动创建当前状态快照到 recovery 目录，health check 失败时自动回滚
- 恢复操作在 systemd transient service 中执行，脱离终端会话生命周期
- 恢复后必须做健康检查（端口可达性）
- 多文件替换采用 all-or-nothing 语义：先解压到 staging，替换完成后再删除 staging；中途失败时保持原始文件不变

## 8. i18n 消息

需新增以下消息键：

| 键 | 内容 |
|---|---|
| `backup.created` | 备份创建成功: {id} |
| `backup.restored` | 恢复成功 |
| `backup.restore_started` | 恢复已启动，查看进度: journalctl -u lkit-restore -f |
| `backup.restore_failed_rolled_back` | 恢复失败，已自动回滚到原版本 |
| `backup.rebuilt` | 已解压到 {path} |
| `backup.deleted` | 备份已删除 |
| `backup.not_found` | 未找到备份: {id} |
| `backup.no_process` | Landscape 进程未运行，请先启动服务 |
| `backup.space_insufficient` | 磁盘空间不足 |
| `backup.recovery_snapshot_created` | 当前版本快照已创建 |
| `backup.confirm_delete` | 确认删除备份 {id}？ |

## 9. 与现有 spec (v0.1 draft) 的差异

| 项目 | 原设计 | 当前设计 |
|---|---|---|
| 备份内容 | landscape.toml + db + init.lock + static | **binary + static + init TOML**（API导出） |
| 格式 | 自定义 magic header 容器 | **标准 tar.gz** |
| 索引 | 依赖 Landscape 维护 backup_index.json | **不依赖**，内容固定 |
| DB 备份 | 默认包含 | **不备份**，init 配置已包含核心配置 |
| 恢复语义 | 原地恢复文件+DB | **离线重建**，从 init 配置重新初始化 |
| 二进制来源 | 未单独定义 | **从运行进程 /proc/*/exe 发现** |
| 备份时停服务 | 需要 | **不需要**（运行时 binary/static 只读） |
| 版本检查 | 未定义 | **无版本检查，全自动 rollback 保护** |
| SSH 断连保护 | 未定义 | **systemd-run 托管进程，断连不中断** |
| health check 失败处理 | 未定义 | **自动从 recovery 快照恢复原版本** |
| 自动/手动标记 | 无 | metadata 加 `auto: bool` 字段 |
| 重建命令 | 无 | 新增 `lkit backup rebuild` |
