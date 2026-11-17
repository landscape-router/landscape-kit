# `lkit restore`

`restore` 在一个已经存在且状态有效的安装内恢复指定 `.lkb`。它不下载仓库资产，也不受
`switch` 的“只允许升级到更高版本”限制；目标版本由备份 metadata 决定，可以与当前版本
相同、较低或较高。

```text
lkit restore (--backup <ID> | --file <PATH>)
             [--allow-no-backup] [--yes]
```

`--non-interactive` 和 `--lang` 是全局参数。交互模式必须确认当前版本、目标版本、备份 ID
和 minimal scope 的数据损失；非交互模式必须额外提供 `--yes`，否则直接返回参数错误。
`--yes` 在非交互模式下确认恢复计划与 minimal scope 数据损失。

`.lkb` 包含网络配置，所有恢复均有独立的网络确认窗口。`--yes` 只确认开始恢复，
不代替应用配置后的连通性确认。本机健康检查通过后，操作进入 `awaiting_confirmation`，
CLI 返回 `0` 表示正在等待确认；用备份中的管理地址重新连接并检查业务后执行
`lkit network confirm`，或在控制台确认。`lkit network rollback` 主动恢复原现场。
超时或确认前整机重启由 daemon 自动回滚；仅重启 daemon 不延长期限。

landscape 根从 `install-state.json` 发现，命令不接收 `--install-dir`；`--backup <ID>`
解析 lkit 地盘备份目录中的备份，`--file <PATH>` 指向外部复制的 `.lkb`。

CLI 与 TUI 负责确认，daemon 直接执行持久化 restore 操作，不启动子命令进程。
前端提交安装根、当前版本、完整安装状态摘要、备份位置及已验证字节的 SHA-256；
daemon 持锁后复核安装现场和备份摘要。现场或备份变化时拒绝执行，要求重新确认。
目标备份读取并验证后以同一份字节执行，不再次读取原路径来选择内容。
控制台完整校验产生的确认上下文包含同一份安装状态和备份摘要，并直接传给提交入口；
选择项与校验结果不一致时要求重新校验。单独传 `--console-confirmed` 不构成确认，
缺少内存上下文时返回参数错误，不能重新读取现场来替代用户已确认的内容。

确认发生在事务创建之前：用户拒绝或缺少 `--yes` 时不创建事务、不写任何文件（`--file`
也不产生暂存拷贝），服务与现场保持不变，分别返回 `1` 和 `2`。`--backup <ID>` 只接受
备份 ID 格式 `YYYYMMDD-HHMMSS-<8位小写hex>`，其他取值视为参数错误（`2`）。

## 恢复前检查

恢复只接受已有有效 `install-state.json` 的安装，不负责空目录安装或新机器灾难重建。
daemon 在创建恢复事务前检查服务注册归属、服务定义和恢复启动检查。systemd、OpenRC
与 SysVinit 的 Landscape 服务均须在启动前执行 `lkit network startup-check`，防止
主机重启后运行尚未确认的配置。服务定义缺少此检查时，恢复拒绝执行并提示先运行
[`lkit repair service`](repair.md#服务定义修复)，修复成功后重新确认恢复计划；
`--yes` 与 `--allow-no-backup` 均不能跳过这项检查。拒绝时不创建恢复事务、保护备份，
也不停止服务或修改安装现场。

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

控制台观察持久化操作状态，全屏操作页显示步骤进度条：准备（`1/4`）、停止服务
（`2/4`）、激活（`3/4`）、初始化与健康检查（`4/4`）。
restore 不下载仓库资产，进度条表示阶段进度而非字节进度。

## 激活与提交

restore 使用全局安装事务锁，由 daemon 执行。systemd、OpenRC 和 SysVinit 均由安装记录对应的服务后端管理：

1. 记录恢复前的 `current`、state、unit、enabled/active 和 resolv.conf；
2. 将当前 `data/` 原子移动到事务目录，作为中断恢复现场；
3. 从 `.lkb` 重建目标版本 release、空 `data/`、`landscape_init.toml` 和 `geo_tmp`；
4. 原子切换 `current`，注册并启动目标 unit；
5. 完成初始化和完整健康检查后保存待提交 state，开始网络确认窗口；确认后提交目标 state。

确认窗口从本机健康检查通过后开始，时长与网络接管一致。旧 state、data 和 release
恢复依据一直保留；恢复不重新停止或接管宿主网络服务。确认动作检查本机健康并再次
核对截止时间，用户负责从恢复后的网络验证访问。确认决定持久化后，即使 daemon 或
主机中断也继续完成提交；未确认时恢复原 data、release、服务状态及 DNS 文件。
该能力需要支持 `restore-v2` 的 daemon，旧 daemon 在提交前被拒绝并提示升级。


恢复成功后，事务目录中的旧 data 现场保留用于诊断和中断恢复。`.lkb` 不包含 SQLite
数据文件，但包含 `landscape_init.toml`：恢复后首次启动时 Landscape 会清空并重建
数据库（init 文件只能被同版本二进制消费；restore 的二进制与 init 文件来自同一备份，
版本恒等），详见 [`.lkb` 备份与回滚](../backup/lkb-and-rollback.md#landscape_inittoml-与数据库重建)。
API token、日志和指标不包含，备份之后新增的数据会丢失。

## 失败与恢复

- 激活或健康检查失败，自动恢复成功：事务为 `rolled_back`，返回 `5`。
- 自动恢复失败：保留未完成事务和恢复依据，操作为 `recovering`，观察者返回 `6`；daemon 重试恢复并阻止后续修改。
- 准备阶段失败时不改变运行状态；停止阶段失败先恢复原服务状态。
- 参数错误返回 `2`，用户拒绝或普通失败返回 `1`。
- Ctrl+C 只退出观察，返回 `130`，不取消 daemon 操作。显式取消只接受尚未领取的排队操作；restore 没有下载阶段。
- daemon 重启按关联事务恢复；已提交事务补写成功结果，不重复创建保护备份或再次恢复目标。

受管后端自动回滚的顺序固定为：停止目标服务 → 恢复 unit 注册与 enabled 状态 →
（同版本 restore 时把被替换的原 release 从事务目录移回）→ 恢复 `current` 与 `data/` →
仅在恢复前服务活跃时启动并做完整健康检查 → 重新提交恢复前 state。同版本 restore 回滚
后，release 内容与回滚前完全一致（不是备份内二进制/静态资源的重建版本）。

恢复不得伪造成功，也不得在没有必要事实时猜测 service manager 或 `current`。
恢复提交的 state 中：`active_version` 取备份 metadata；`webserver` 身份从解包二进制
现场计算；`static_archive` 身份从备份内 `static.zip` 现场计算——该身份不要求与任何
仓库 manifest 一致（备份内的 `static.zip` 是备份时从 `current/static/` 现场打包的），
恢复内容即备份快照（含自定义前端页面）。恢复后如需回到仓库身份，运行
`repair static`（自定义前端源激活时按激活源意图恢复）。restore 不下载仓库资产，
不读取也不改写 `config.toml`。

