# `lkit restore`

`restore` 在一个已经存在且状态有效的安装内恢复指定 `.lkb`。它不下载仓库资产，也不受
`switch` 的“只允许升级到更高版本”限制；目标版本由备份 metadata 决定，可以与当前版本
相同、较低或较高。

```text
lkit restore (--backup <ID> | --file <PATH>)
             [--install-dir <PATH>] [--allow-no-backup] [--yes]
```

`--non-interactive` 和 `--lang` 是全局参数。交互模式必须确认当前版本、目标版本、备份 ID
和 minimal scope 的数据损失；非交互模式必须额外提供 `--yes`，否则直接返回参数错误。
`--yes` 覆盖全部确认：在非交互模式下同时表示确认恢复计划、minimal scope 数据损失以及
（none 模式）外部实例已由用户自己的进程管理器停止。

从交互控制台（TUI）发起的 restore 由 TUI 恢复确认层完成全部确认，命令内部标记
`--console-confirmed`，不再请求 `/dev/tty` 二次确认；这在 systemd worker 路径下是必需
的——worker 是独立进程且无法读取 TUI 的键盘输入，继续交互确认会阻塞在输出提示上。

确认发生在事务创建之前：用户拒绝或缺少 `--yes` 时不创建事务、不写任何文件（`--file`
也不产生暂存拷贝），服务与现场保持不变，分别返回 `1` 和 `2`。`--backup <ID>` 只接受
备份 ID 格式 `YYYYMMDD-HHMMSS-<8位小写hex>`，其他取值视为参数错误（`2`）。

## 恢复前检查

恢复只接受已有有效 `install-state.json` 的安装，不负责空目录安装或新机器灾难重建。
在停止服务前完成：

1. 解析并完整验证目标 `.lkb`（header、metadata、checksum）；
2. 检查备份架构与当前主机和安装架构一致；
3. 交互确认恢复计划与 minimal scope 数据损失（非交互模式以 `--yes` 代替）；
4. 默认创建当前实例的保护 `.lkb`，并将其记录在 restore 事务中；
5. 在事务临时目录安全解包归档并完成完整内容校验：`landscape-webserver`、
   `landscape_init.toml` 与 `static.zip` 必须为普通文件，`static/` 与 `geo_tmp/` 必须为
   目录，解包目录与文件分别保持 `0700`/`0600`。

目标备份损坏、架构不匹配、归档缺少必要内容或保护备份创建失败时，保持当前服务和现场
不变。保护备份带固定备注（`restore 前自动保护备份`，auto 标记为 true）。
`--allow-no-backup` 只允许在保护备份无法创建时继续，明确表示不产生可移植的当前
配置快照；它不跳过目标备份校验或用户确认。

解包在停止服务前完成，解包结果供激活阶段直接使用；解包过程不修改运行态，也不会在
`/tmp` 或任何可预测路径留下中间产物。

systemd 模式委托 worker 执行时，全屏操作页显示步骤进度条：准备（`1/4`）、停止服务
（`2/4`）、激活（`3/4`）、初始化与健康检查（`4/4`）；none 模式为两步（准备、激活）。
restore 不下载仓库资产，进度条表示阶段进度而非字节进度。

## 激活与提交

restore 使用与 switch 相同的事务锁和 systemd operation worker。systemd 模式下：

1. 记录恢复前的 `current`、state、unit、enabled/active 和 resolv.conf；
2. 将当前 `data/` 原子移动到事务目录，作为中断恢复现场；
3. 从 `.lkb` 重建目标版本 release、空 `data/`、`landscape_init.toml` 和 `geo_tmp`；
4. 原子切换 `current`，注册并启动目标 unit；
5. 完成初始化和完整健康检查后提交目标 state。

`none` 模式要求用户通过 `/dev/tty` 确认外部实例已停止（非交互模式以 `--yes` 代替）。
lkit 不启动、不探测外部进程，恢复后提交 `initialization.status: pending`、
`service.verified: false`，并输出参考启动命令。

恢复成功后，事务目录中的旧 data 现场保留用于诊断和中断恢复。`.lkb` 不包含 SQLite
数据文件，但包含 `landscape_init.toml`：恢复后首次启动时 Landscape 会清空并重建
数据库（init 文件只能被同版本二进制消费；restore 的二进制与 init 文件来自同一备份，
版本恒等），详见 [`.lkb` 备份与回滚](../backup/lkb-and-rollback.md#landscape_inittoml-与数据库重建)。
API token、日志和指标不包含，备份之后新增的数据会丢失。

## 失败与恢复

失败语义按 service manager 区分：

- systemd 模式：
  - 目标激活或健康检查失败，但恢复前状态自动恢复成功：事务为 `rolled_back`，返回 `5`；
  - 自动恢复失败或事务/状态损坏：事务为 `failed`，保留目标、旧 data 和备份，返回 `6`；
  - 停止目标服务失败（服务状态可能已改变）：先按 `systemd_before` 恢复 unit 注册与
    enabled/active 状态，再标记 `failed`；恢复成功返回普通失败 `1`，恢复也失败时按
    自动恢复失败处理，返回 `6`；
- none 模式：lkit 不启动、不探测外部进程，因此激活后的失败不内联自动回滚，返回普通
  失败 `1`；现场（`previous-data`、`previous_current`、previous-state 和保护备份）保留
  在事务目录，下次任意 lkit 命令通过中断恢复入口按原阶段恢复原安装；
- 参数错误返回 `2`，用户拒绝或普通失败返回 `1`，显式 Ctrl+C 返回 `130`。

systemd 模式自动回滚的顺序固定为：停止目标服务 → 恢复 unit 注册与 enabled 状态 →
（同版本 restore 时把被替换的原 release 从事务目录移回）→ 恢复 `current` 与 `data/` →
仅在恢复前服务活跃时启动并做完整健康检查 → 重新提交恢复前 state。同版本 restore 回滚
后，release 内容与回滚前完全一致（不是备份内二进制/静态资源的重建版本）。

恢复不得伪造成功，也不得在没有必要事实时猜测 service manager 或 `current`。
恢复提交的 state 中：`active_version` 取备份 metadata；`webserver` 身份从解包二进制
现场计算；`static_archive` 身份从备份内 `static.zip` 现场计算。restore 不下载仓库资产，
不读取也不改写 `config.toml`。

