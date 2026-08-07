# `lkit backup`

`backup` 管理可长期保留的 `.lkb` minimal 配置级备份。它与 `switch` 的职责不同：
`switch` 只为版本升级创建自动备份，`backup` 允许用户主动创建、查看和验证备份。

```text
lkit backup create [--remark <TEXT>] [--output <PATH>]
                    [--install-dir <PATH>]
lkit backup list [--install-dir <PATH>]
lkit backup show (--backup <ID> | --file <PATH>) [--install-dir <PATH>]
lkit backup verify (--backup <ID> | --file <PATH>) [--install-dir <PATH>]
```

`--non-interactive` 和 `--lang` 仍是全局参数，可放在子命令前后。

控制台（裸 `lkit`）在 Backup 面板提供同样的能力：后台执行与 `backup list` 相同的解析与
完整校验列出备份，Enter 查看 metadata 详情，R 进入恢复确认，V 执行完整 verify；面板顶部
的“创建备份”动作允许输入备注后创建，恢复通过确认层后由控制台把结构化 `Restore` 请求交给
共享命令分发（systemd 模式仍委托 worker 执行）。面板在未安装或非 root 时明确提示不可用。

## `backup create`

创建备份不会停止、启动或重启 Landscape，也不改变 `current`、安装状态或仓库来源。
命令仍需取得安装锁，避免与 switch、repair、restore 或 service-manager 同时运行。

创建前必须满足：

- 安装状态完整且当前版本与 `current` 一致；
- 当前实例正在运行，并能通过固定配置导出 API 返回与活动版本一致的配置；
- 当前运行二进制、`static/` 和 `geo_tmp/` 能按 `.lkb` v1 规则读取；
- 当前架构为 `x86_64` 或 `aarch64`。

`systemd` 模式使用受管服务；`none` 模式要求用户自己的进程管理器已经启动 Landscape。
无法导出配置、token 不安全、配置版本不一致或归档自校验失败时，不生成最终备份文件。

默认备份写入：

```text
<install-root>/backups/<backup-id>.lkb
```

`--output <PATH>` 将备份原子写入指定的新文件。目标文件不得已存在、不得是符号链接，
最终权限为 `root:root`、`0600`。`--remark` 是最多 256 个字符的单行说明，不得包含控制
字符；手工备份写入 metadata 的 `auto: false`。

未提供 `--remark` 时：交互模式在创建前通过 `/dev/tty` 提示输入备注，空回车表示不写
备注；非交互模式（`--non-interactive` 或无法打开终端）缺省为空。无论来自参数还是
交互输入，备注都统一校验（最多 256 字符、单行、无控制字符），非法时返回参数错误 `2`。

备份包含后端二进制、当前静态页面、该版本官方静态压缩包 `static.zip`、导出的
`landscape_init.toml` 和 `geo_tmp`，不包含：

- `landscape_db.sqlite` 及其他数据库文件；
- `landscape_api_token`；
- 日志、指标、socket 和其他运行时文件。

因此它是配置级恢复能力，不是数据库级灾难备份。

## `backup list`

只枚举安装根目录下的普通 `.lkb` 文件，按 `created_at` 从新到旧排列。输出至少包含备份
ID、创建时间、Landscape 版本、架构、`auto`、scope、remark 和 metadata 状态。除内容校验
外，每个条目还执行与 `show`/`verify` 相同的安全校验：必须为 root 所有、权限不宽于
`0600` 的普通文件。损坏、权限或所有者不安全以及符号链接条目显示为 invalid 并使命令
返回普通失败；符号链接不会被跟随。

## `backup show` 与 `backup verify`

`--backup <ID>` 只解析安装根目录的备份 ID，ID 必须符合备份 ID 格式
`YYYYMMDD-HHMMSS-<8位小写hex>`，其他取值视为参数错误；`--file <PATH>` 用于检查外部
复制的备份。路径必须指向 root 所有、权限不宽于 `0600` 的普通文件。

`show` 展示 metadata 和备份边界；`verify` 额外完整读取并校验 header、metadata、零填充、
tar.gz checksum、归档路径、条目类型和内容完整性，但不会改变安装现场。内容完整性要求
与 restore 相同：`landscape-webserver`、`landscape_init.toml` 与 `static.zip` 必须为
普通文件，`static/` 与 `geo_tmp/` 必须为目录；缺失任一必需条目的备份拒绝通过。
verify 解包到随机命名的 `0700` 临时目录，解包目录与文件分别保持 `0700`/`0600`，
不使用可预测路径，也不会在 `/tmp` 留下中间产物。

已有 v1 `.lkb` 始终可以 verify。所有 v1 备份都携带 `static.zip`，restore 可以从备份
内容现场计算静态资产身份，不需要在 metadata 中记录仓库来源。
