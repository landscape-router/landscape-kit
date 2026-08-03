# `.lkb` 备份与回滚

## `.lkb` 配置级备份

### 目的与语义

正常版本切换和需要重启的后端 repair 必须通过运行中的 Landscape 导出当前配置，并创建
`.lkb` 备份。唯一例外是 systemd 管理的服务已经停止，且用户对版本切换显式指定
`--allow-no-backup`；后端 repair 不允许该例外。服务正在运行时，该参数只产生警告，
不会跳过备份。

`.lkb` 恢复是配置级重建，不是数据库字节级回滚：

- 不复制 `landscape_db.sqlite`；
- 恢复时创建新的空 data 目录；
- 使用导出的 `landscape_init.toml` 重新初始化同版本 Landscape；
- 保证核心配置一致，不保证恢复日志、指标、缓存状态或 API token。

### 配置导出

需要创建备份时，必须调用本机固定接口：

```text
GET https://127.0.0.1:6443/api/v1/system/config/export
Authorization: Bearer <token>
Accept: application/json
```

允许自签名证书。HTTP 成功响应必须是 UTF-8 JSON，最小结构为：

```json
{
  "data": {
    "filename": "landscape_init_v0.19.2.toml",
    "version": "0.19.2",
    "content": "version = \"0.19.2\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"...\"\n"
  }
}
```

外层字段规则：

- HTTP 状态必须为 `200`；
- `data` 必须是非 null 对象；
- `error_id`、`message` 和 `args` 可以缺失或为 null；任何非 null `error_id` 均视为业务失败；
- 未知外层字段允许忽略。

`data` 字段规则：

- `filename`、`version`、`content` 都是必填字符串；
- `version` 必须是规范化后与当前运行版本一致的 SemVer；
- `filename` 必须等于 `landscape_init_v<version>.toml`，只用于一致性校验，不作为归档路径；
- `content` 是完整 TOML 字符串，按原字节写入归档根目录的 `landscape_init.toml`；
- 未知 data 字段允许忽略。

API token 固定存放在当前 Landscape home path：

```text
<install-root>/data/landscape_api_token
```

读取规则：

- 必须是 root 所有的普通文件，不跟随符号链接；
- 文件权限不得宽于 `0400`，group 和 other 不能拥有任何权限；
- 文件大小必须在 `1..=1 MiB`，按 UTF-8 读取；
- token 读取后只移除一个行尾：末尾为 `\r\n` 时移除这两个字节，否则末尾为 `\n` 时只移除该字节；不 trim 其他空白。Landscape 自身生成的文件没有行尾，该规则同时兼容通过 `echo` 或文本工具写入的文件；
- 移除允许的单个行尾后，token 必须非空且不能再包含 `\r`、`\n` 或其他 ASCII 控制字符；
- 作为 `Authorization: Bearer <token>` 仅保存在内存中，请求完成后尽快释放；
- 文件缺失、权限过宽、为空、非法 UTF-8 或认证失败时中止备份；
- token 不得写入 `.lkb`、事务、状态、安装日志或错误详情。

API 返回中的 `content` 才是备份归档内的 `landscape_init.toml`。不得复制安装时保留的原始初始化文件作为运行态备份。

当前 Landscape 导出配置包含 `config.auth.admin_user` 和 `config.auth.admin_pass`，因此 `.lkb` 包含可用于恢复的管理员凭据，属于高敏感备份。导出配置不包含每次启动生成的 `landscape_api_token`；该 token 只能临时用于调用导出 API，不得进入 `.lkb`。`.lkb`、生成过程中的临时文件和解包目录必须始终为 `root:root` 且文件权限不宽于 `0600`、目录权限不宽于 `0700`，日志不得输出导出 TOML 内容。

在需要备份的版本切换或后端 repair 中，以下任一情况必须中止操作：

- API 无法认证或连接；
- HTTP 或业务响应失败；
- 响应字段缺失或格式错误；
- API 返回版本、TOML 中版本和当前运行版本不一致；
- TOML 无法解析或验证；
- 归档所需文件无法完整读取。

### v1 minimal 内容

v1 只允许 `scope: "minimal"`，不定义或接受 `full`。

归档固定包含：

```text
.
├── landscape-webserver
├── landscape_init.toml
├── static/
│   └── 当前实际静态页面
└── geo_tmp/
    ├── ip/
    └── site/
```

内容来源：

- `landscape-webserver`：当前实际运行且通过状态摘要验证的二进制；
- `landscape_init.toml`：通过导出 API 获得的完整当前运行态配置；
- `static/`：当前实际目录，包括用户自定义页面；
- `geo_tmp/`：当前 Landscape home 下的 GeoIP/GeoSite 数据缓存。

`geo_tmp` 不存在时允许备份，归档中创建空目录并报告 warning。目录存在但不能完整读取时备份失败并中止升级。

v1 不跟随归档源中的符号链接，只允许目录和普通文件。发现符号链接、设备文件、FIFO 或 socket 时失败。

明确排除：

- `landscape_db.sqlite`；
- `landscape_api_token`；
- `logs/`；
- `metric/`；
- `hostapd_tmp/`；
- Unix socket；
- 不存在的 `landscape_backup_index.json`。

### 容器格式 v1

```text
偏移 0:          32 字节二进制 Header
偏移 32:         json_len 字节 UTF-8 BackupMetadata JSON
其后:            全零填充至 1 MiB
偏移 1048576:    gzip 压缩的 tar 归档，直到 EOF
```

Header：

| 偏移 | 长度 | 字段 |
| ---: | ---: | --- |
| `0` | 4 | ASCII magic `LKB1` |
| `4` | 2 | 容器版本，`u16 LE`，v1 为 `1` |
| `6` | 4 | `json_len`，`u32 LE` |
| `10` | 6 | `reserved1`，必须全零 |
| `16` | 16 | `reserved2`，必须全零 |

规则：

- `json_len > 0`；
- `json_len <= 1048576 - 32`；
- JSON 后到 1 MiB 的填充必须全零；
- reserved 非零时 v1 读取器拒绝；
- 未知容器版本拒绝；
- 文件长度必须大于 1 MiB；
- tar 路径必须相对、规范化且不能逃逸目标目录；
- gzip/tar 解码失败或包含不允许类型时拒绝恢复。

### BackupMetadata Schema v1

```json
{
  "schema_version": 1,
  "backup_id": "20260801-163000-a1b2c3d4",
  "created_at": "2026-08-01T16:30:00Z",
  "landscape_version": "0.19.2",
  "lkit_version": "0.3.0",
  "architecture": "x86_64",
  "hostname": "router01",
  "remark": "升级前自动备份",
  "auto": true,
  "scope": "minimal",
  "contents": {
    "binary": true,
    "static": true,
    "init_config": true,
    "geo_cache": true
  },
  "checksum": "sha256:ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890"
}
```

字段规则：

- `schema_version` 固定为整数 `1`；
- `backup_id` 格式为 `{YYYYMMDD-HHMMSS}-{tar.gz SHA-256 前 8 位}`，其中时间部分固定使用 UTC，不使用主机本地时区；
- `created_at` 为 UTC RFC 3339，并与 `backup_id` 的 UTC 时间表示同一次创建操作；
- `landscape_version` 为规范化 SemVer；
- `architecture` 只允许 `x86_64` 或 `aarch64`；
- `remark` 是字符串，可以为空；
- `auto` 表示是否为安装器自动创建的升级前备份；
- `scope` v1 只能为 `minimal`；
- `contents` 四个已定义布尔字段必须为 true；
- `checksum` 为 `sha256:` 加 64 位小写十六进制字符，校验偏移 1 MiB 到 EOF 的完整 tar.gz 字节；
- 未知字段允许忽略；必填字段缺失或非法时拒绝。

`checksum` 用于发现损坏，不是数字签名，不能证明来源可信。

### 创建顺序与存放

备份路径：

```text
<install-root>/backups/<backup-id>.lkb
```

创建流程：

1. 在 `backups/.tmp/` 流式生成 tar.gz；
2. 计算 tar.gz SHA-256；
3. 生成 `backup_id` 和 Metadata；
4. 写入 Header、JSON、零填充和 tar.gz；
5. 将临时 `.lkb` 完整重新读取并验证；
6. 设置 `root:root 0600`；
7. 原子移动到最终路径。

同名备份不得覆盖。自动备份和未来手工备份使用相同格式。v1 永久保留。

在需要备份的路径中，只有 `.lkb` 完整落盘并自校验成功后，版本切换事务才能停止
Landscape。

事务文件只记录 `.lkb` 相对路径、backup ID 和整个 `.lkb` 文件 SHA-256，不重复复制备份内容。

## 无备份切换的失败恢复

systemd 服务已经停止且用户显式指定 `--allow-no-backup` 时，事务记录 `no_backup: true`
且不记录 `backup`。目标版本激活失败后，`lkit` 仍停止目标进程并恢复旧 `current`、unit
注册、enabled/active 状态和 `/etc/resolv.conf`；切换前服务本来就是停止状态，因此恢复后
不启动旧版本。该路径不创建空 data、不执行配置级重建，也不能恢复目标版本可能已经修改
的数据。具体命令约束见 [`lkit switch`](../commands/switch.md)，事务恢复规则见
[事务与中断恢复](../deployment/transactions-and-recovery.md)。

## `.lkb` 回滚流程

systemd 环境中目标版本启动或健康检查失败时：

1. 将事务标记为 `rolling_back`；
2. 停止失败版本；
3. 保留失败日志和现场；
4. 将失败后的 `data/` 原子移动到 `transactions/<transaction-id>/failed-data/`；
5. 创建新的空 `data/`；
6. 在事务临时目录校验并安全解包 `.lkb`；
7. 用备份内二进制和静态目录重建 `releases/<from_version>`；若该目录已存在，先移动到 `transactions/<transaction-id>/replaced-release/`，不得原地覆盖；
8. 将备份内 `geo_tmp/` 恢复到新 `data/geo_tmp/`；
9. 将 API 导出的配置写为新 `data/landscape_init.toml`，权限 `0600`；
10. 确保新 data 中不存在数据库、`landscape.toml` 和 `landscape_init.lock`；
11. 原子恢复 `current -> releases/<from_version>`；
12. 使用备份内同版本、同架构二进制启动；
13. Landscape 从导出配置重建数据库和持久配置；
14. 执行完整健康检查；
15. 以恢复后实际资产摘要、初始化完成状态和旧仓库来源重新提交旧版本安装状态；
16. 将事务标记为 `rolled_back`。

只有旧版本重新初始化并通过健康检查时，才能报告自动回滚成功。

回滚不会恢复：

- 原 SQLite 文件和非导出数据库状态；
- API token；
- 日志；
- 指标历史；
- Unix socket；
- 未包含在 `InitConfig` 和 `geo_tmp` 中的运行态数据。

回滚失败时：

- 标记事务 `failed`；
- 保留 `.lkb`、失败 data、目标版本和旧版本；
- 返回退出码 `6`；
- 输出人工恢复所需路径和阶段；
- 不继续自动重试。

版本切换或后端 repair 失败但自动回滚成功时返回退出码 `5`，因为用户请求的目标状态未成功激活。
