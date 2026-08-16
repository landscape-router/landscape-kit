# 扩展 Docker 功能 E2E 场景

## 目标与约束

Docker 功能 E2E 在 install、成功 switch 和健康失败回滚的基础上覆盖以下扩展场景。
约束与基础场景一致：

- 被测 `lkit` 使用 `test-support` 构建，通过 `--test-runtime` 显式选择
  `preflight: skip`、`execution: inline`；test-support 运行时覆盖 lkit 地盘与
  landscape 安装根（生产 CLI 固定为 `/root/.lkit/` 与 `/root/.lkit/landscape`，
  不提供覆盖参数）；
- 使用 fake systemctl 和隔离的 unit/resolv.conf 路径，但仍绑定固定端口
  `53/6300/6443` 并启动真实 fixture 进程；
- fixture 只通过版本化的 `static/lkit-fixture.json` 切换声明式场景；
- 每个新 release 使用 `lkit-fixture-release --stamp-version` 获得唯一二进制摘要；
  fixture 只编译一次。

全局 unit 只能属于一个安装根：场景以主安装根的全生命周期为主，latest 通道与
导出失败回滚场景先 `lkit uninstall`（保留 lkit 地盘元数据）释放注册链接与端口，
再在串行的新根上安装目标版本，不并行使用多个根。

本文档记录 `run-scenarios.sh` 当前执行的场景和断言。

## 场景总览

| 编号 | 场景 | fixture 场景 | 前置状态 | 命令 | 核心验证点 |
| --- | --- | --- | --- | --- | --- |
| 基础生命周期 | 首次安装 1.0.0 | healthy | 无 | `lkit install --version 1.0.0` | state、服务身份、API token `0400`、install 不创建 config.toml |
| S1 | repair 全流程 | healthy | 已安装 1.0.0 | `lkit repair binary/static` | 从仓库恢复二进制与静态页，SHA 一致 |
| 2.0.0 | 成功切换 | healthy | 已运行 1.0.0 | `lkit switch 2.0.0` | committed、`.lkb` 元数据 |
| 3.0.0 | 健康失败回滚 | health_error | 已运行 2.0.0 | `lkit switch 3.0.0` | 激活阶段失败自动回滚到 2.0.0 |
| S3 | 失败启动矩阵 | start_exit / exit_during_stability / delayed_ready | 已运行 2.0.0 | `lkit switch 4.1.0/4.2.0/4.3.0` | 退出码 5、回滚、rolled_back、服务身份、resolv.conf 恢复 |
| S10 | 手工备份与恢复 | healthy | 已运行 2.0.0 | `lkit backup create/list/show/verify` + `lkit restore` | 手工 minimal 备份、同版本 restore、保护备份 |
| S4 | 停止服务后切换 | healthy | 2.0.0 停止 | `systemctl stop` + `lkit switch 5.0.0 [--allow-no-backup]` | 停止服务默认拒绝；显式允许后无备份切换 |
| S9 | reconcile | healthy | 已运行 5.0.0 | `lkit reconcile` | 受管元数据外部变化校验与拒绝 |
| S11 | restore 激活失败自动回滚 | delayed-ready 2500ms | 8.0.0 运行中 | `lkit restore`（2000ms 超时运行时） | RST-03：激活超时、退出码 5、健康旧版本重启 |
| S12 | restore 中断 phase 恢复 | healthy | 恢复目标激活中 | kill restore 进程 + `lkit reconcile` | RST-05：事务停 verifying、下次命令恢复、data 恢复 |
| S13 | systemd 跨版本 restore | healthy | 已运行 5.0.0 | `lkit restore --backup <2.0.0>` | RST-02：不经过仓库下载、事务形状、config.toml 不变 |
| S14 | 可信残留 release 复用 | delayed-ready 2500ms | 2.0.0 运行中 | `lkit switch 9.0.0`（2 秒超时失败后默认超时重试） | INS-11/SW-11/UP-09：回滚残留目录直接复用、不重写 |
| S7 | latest 通道安装 | healthy | 服务已停止、端口空闲 | 卸载主根 + `lkit install`（无版本） | 解析 stable、多根轮换、install 不修改 config.toml |
| S8 | 中断事务恢复 | healthy | 手工制造 preparing 现场 | `lkit switch 10.0.0` | 确定性恢复未完成事务 |
| S2 | 导出失败回滚 | export_error | 4.0.0 运行中 | `lkit switch 4.1.0` | 备份阶段失败的不同回滚路径 |

版本规划（避免与现有 1.0.0/2.0.0/3.0.0 冲突）：

```text
1.0.0 healthy（基础生命周期安装）
2.0.0 healthy（成功切换；S3/S4/S13 回滚目标）
3.0.0 health_error（健康失败回滚）
4.0.0 export_error（S2 前置，唯一安装根）
4.1.0 start_exit（S3a；S2 的失败切换目标）
4.2.0 exit_during_stability
4.3.0 delayed_ready（ready_delay_ms = 10000，超过 4 秒测试启动超时）
5.0.0 healthy（S4/S9/S11/S13 共用）
8.0.0 delayed_ready（ready_delay_ms = 2500，S11 前置）
9.0.0 delayed_ready（ready_delay_ms = 2500，S14 前置；S7 的 latest 目标）
10.0.0 healthy（S8 目标）
```

## S1 repair 全流程

前置：`1.0.0` 已安装并运行（复用现有 `assert_service_identity`）。

1. 篡改二进制：复制 `releases/1.0.0/landscape-webserver` 到临时文件，追加字节后原子
   替换原路径，使 SHA 与 `state.assets.webserver.sha256` 不一致，避免直接写入正在执行的
   ELF；
2. 执行 `lkit repair binary`，从仓库重新下载并校验：
   - 退出码 `0`；
   - `sha256sum releases/1.0.0/landscape-webserver` 与状态记录一致；
   - 修复创建版本为 `1.0.0` 的 `.lkb`（`assert_backup_metadata`）；
   - `assert_service_identity` 通过（服务身份与 HTTPS 探针）；
3. 删除 `current/static/` 下的页面文件，执行 `lkit repair static`：
   - `current/static/index.html` 与 `current/static/lkit-fixture.json` 恢复；
   - 服务仍运行（MainPID 不变），事务 phase 为 `committed`，不创建 `.lkb`。

## S2 导出失败回滚（export_error）

配置导出 API 由**运行中**的服务提供，`.lkb` 备份的内容来自该次导出。因此
export 失败（发生在备份创建之前）只能在 export_error 版本**正在运行**时被触发；
切换到 export_error 版本本身会成功（fixture 可正常启动，`/api/docs` 返回 `200`）。

单实例约束下该场景在 latest 根卸载后安装 `4.0.0`（export_error）运行：

1. `lkit uninstall --yes`（释放全局 unit 与端口），再 `lkit install --version 4.0.0`；
2. 尝试切换到尚未安装的 `4.1.0`：`lkit switch --version 4.1.0`；
3. 必须断言：
   - 命令返回失败退出码；
   - active version 与 `current` 仍为 `4.0.0`；
   - 最新 switch 事务 phase 为 `failed`（导出失败发生在停止服务之前，没有回滚发生）；
   - 服务仍为 `4.0.0` 且健康；
   - 不产生新的 `.lkb`。

与 `health_error`（激活阶段失败）不同，export 失败发生在**备份创建之前**，事务在
`preparing` 阶段直接标记 `failed`。失败前 `build_release` 已把目标 release 目录
下载落盘（下载先于导出），因此该场景不要求目标 release 目录不存在。

## S3 失败启动矩阵

三个版本分别验证三条失败路径，断言模式与 rollback.md 一致
（退出码 `5`、回滚到 `2.0.0`、事务 phase `rolled_back`、`.lkb` 元数据、
服务身份、`/etc/resolv.conf` 恢复），另加各自特有断言：

| 版本 | fixture 场景 | 特有断言 |
| --- | --- | --- |
| `4.1.0` | `start_exit` | systemd start 后进程立即退出；回滚后服务恢复为 `2.0.0` |
| `4.2.0` | `exit_during_stability` | 就绪后稳定观察期退出被检出；回滚完整 |
| `4.3.0` | `delayed_ready` | `ready_delay_ms=10000` 使启动轮询超过测试运行时 4 秒超时；不残留目标版本进程 |

自动回滚会重新注册并启动 `2.0.0` 的 unit，目标版本 release 目录保留在
`releases/<target>/`（`.lkb` 回滚不清理目标 release）。断言应检查 unit 恢复
enabled/active，且 `current` 与状态都指向 `2.0.0`。

## S4 停止服务后切换

前置：`2.0.0` 运行中。

配置导出 API 由运行中的服务提供，服务停止后无法导出配置快照，因此
`lkit switch` 在受管服务已停止时**默认拒绝**并明确提示：

```text
install: the managed service is not running: ... start it with
`systemctl start landscape-router.service` and retry, or re-run with
--allow-no-backup to switch without a configuration snapshot
```

场景分两步断言：

1. `systemctl stop landscape-router.service`，确认 `is-active` 为 `inactive`；
2. 不带标志执行 `lkit switch --version 5.0.0`：
   - 返回失败退出码，不创建任何事务，active version、`current` 与服务状态不变；
3. 带 `--allow-no-backup` 执行 `lkit switch --version 5.0.0`：
   - 输出明确警告（无配置快照、无 `.lkb`、回滚不能恢复数据）；
   - 切换成功，服务被激活并启动，MainPID 属于 `releases/5.0.0/landscape-webserver`；
   - 事务 phase 为 `committed`，`no_backup: true` 且不记录 `backup`；
   - 不产生新的 `.lkb`。

`--allow-no-backup` 跳过配置导出与 `.lkb` 创建；激活失败时仍自动回滚
（停止目标进程、按 `systemd_before` 恢复 unit 注册与 enabled/active、恢复
`/etc/resolv.conf` 与 `current`，切换前服务在运行时才重新启动旧版本），但无法
恢复被目标版本重新初始化过的数据。服务运行中显式传 `--allow-no-backup` 时忽略
并仍创建 `.lkb`。

## S9 reconcile

`lkit reconcile` 的语义是"检查活动版本并协调受管元数据的外部变化"，
**不会**从缺失状态或损坏链接重建状态。`load_state` 对缺失 `current` 链接或
`current` 漂移直接失败，缺失 `state/install-state.json` 时按首次安装处理并拒绝。
场景按以下语义断言：

1. 在初始化已完成的安装中，外部向 `data/landscape_init.toml` 追加标记，执行
   `lkit reconcile`：
   - 退出码 `0`，文件字节不变，state 不包含初始化文件摘要；
2. 再次执行 `lkit reconcile`（无变化、无需确认）：退出码 `0`，状态文件有效；
3. 删除 `state/install-state.json`，执行 `lkit reconcile`：
   - 命令拒绝（非零退出码），不重建状态文件；恢复状态文件；
4. 破坏 `current` 链接（指向 `releases/2.0.0`），执行 `lkit reconcile`：
   - 命令拒绝激活漂移（非零退出码）；恢复 `current` 后再次执行通过，
     服务身份与 state 一致。

## S10 手工备份与恢复

在运行中的 systemd 实例上创建手工 minimal 备份（`auto: false` + remark），
列出、查看、校验，再同版本 restore：

1. `lkit backup create --remark "manual e2e backup"`；
2. `lkit backup list` 包含该备份；`lkit backup show` 打印 `backup_id`；
   `lkit backup verify` 通过；
3. 同版本 restore（`lkit restore --backup <id> --non-interactive --yes`）：
   - 创建保护备份（`lkb_count` 增加 1）；
   - `install-state.json` 与 `config.toml` 字节不变；
   - 事务 phase `committed`。

## S11 restore 激活失败自动回滚（RST-03）

发布 `delayed-ready` 2500ms 的 `8.0.0`，用默认 4 秒启动超时正常切换并创建其
手工备份；restore 时改用 2000ms 启动超时的运行时，激活必然超时失败，systemd
模式内联自动回滚并返回退出码 5：

1. `run_switch 8.0.0` 成功，`assert_backup_metadata` 记录 5.0.0 自动备份；
2. `lkit backup create --remark "rst03 target backup"` 记录 8.0.0 手工备份；
3. 先用 5.0.0 自动备份降级恢复（switch 拒绝降级，restore 允许），回到健康版本；
4. 以 2000ms 超时运行时 restore 到 8.0.0 的备份：
   - 退出码 5，事务 phase `rolled_back`；
   - 回滚恢复健康旧版本并重启，`assert_service_identity` 通过。

## S12 restore 中断 phase 恢复（RST-05）

恢复目标激活期间 kill 掉 lkit，事务停在 `verifying`，`data` 已移入
`previous-data`；下次命令经 phase 恢复入口完成回滚并恢复原 `data`：

1. 以 `restore-short-startup.json`（2000ms 超时）后台执行 restore；
2. python 轮询事务目录，事务进入 `verifying` 后 kill restore 进程
   （verifying 必然发生在 data 移入 previous-data 之后）；
3. `lkit reconcile` 触发恢复：退出码 0，事务最终 phase `rolled_back`，
   `data/rst05-marker` 恢复（previous data 还原）。

## S13 systemd 跨版本 restore（RST-02）

当前 5.0.0，用 S10 创建的 2.0.0 手工备份降级 restore，不经过仓库下载：

- 创建保护备份（`lkb_count` 增加 1）；
- 事务形状：`operation == restore`、`phase == committed`、
  `from_version == 5.0.0`、`target_version == 2.0.0`、
  `restore_backup.backup_id` 指向所选备份、`backup` 非空、`no_backup == false`；
- `config.toml` 字节不变；
- state 的资产摘要与备份归档内容一致（解压校验）。

## S14 可信残留 release 复用（INS-11/SW-11/UP-09）

失败切换回滚后残留的 `releases/<target>` 目录在再次切换时被直接复用
（可信校验通过），不重复下载、不覆盖。`delayed-ready` 2500ms 在默认 4 秒启动
超时下成功、在 2 秒超时下失败回滚，恰好制造"下载完成但激活失败"的残留目录：

1. 以 2000ms 超时运行时 `switch 9.0.0`：退出码 5，事务 phase `rolled_back`，
   `releases/9.0.0/` 残留；
2. 记录残留目录的二进制 SHA 与文件数；
3. 以默认运行时 `switch 9.0.0`：成功（退出码 0），事务 phase `committed`，
   残留目录的 SHA 与文件集不变（未重写、未覆盖）。

## S7 latest 通道安装

卸载主安装根（释放全局 unit 与端口）后执行 `lkit install --repository <base>`
（不带版本）：

- 解析 `channels/stable.json` 为最新版本（此时为 9.0.0）并安装成功；
- state `active_version == 9.0.0`，仓库来源不写入 state；
- `config.toml`（lkit 地盘）字节不变：install 不创建、不修改它；
- 服务身份与 state 一致。

## S8 中断事务恢复

以确定性方式模拟 kill -9 中断（不真正 kill）：

1. 手工写入一个 switch 事务文件（phase `preparing`，from `9.0.0` → target
   `10.0.0`）到 lkit 地盘事务目录，并制造现场：目标 release 目录
   `releases/10.0.0/` 半成品（只含部分文件）、`current` 未动；
2. 发布 `10.0.0` 后执行 `lkit switch --version 10.0.0`，触发 `recover_interrupted`；
3. 断言：
   - 命令正常完成（退出码 `0`），无残留未完成事务；
   - 手工制造的事务 phase 为 `failed`（preparing 阶段恢复：清理半成品目标
     release 目录并标记失败），新 switch 事务 phase 为 `committed`；
   - 最终 active version 与 `current` 为 `10.0.0`，服务身份与 state 一致；
   - 新切换创建版本为 `9.0.0` 的 `.lkb`。

事务 JSON 格式以 `deployment/transaction` 模块的校验规则为准，场景脚本直接构造
（`schema_version`、`log_path`、`canonical_install_root` 等字段必须合法）。

## 明确不做

- SQLite 数据库级备份、空目录灾难重建和数据库内容恢复；当前 `.lkb` 仍是 minimal 配置级备份；
- 真实 kill -9 崩溃（用 S8 的确定性现场模拟替代）；
- QEMU 与完整宿主 preflight；systemd-nspawn 另有低频兼容性 smoke test；
- 委托执行路径（daemon 托管）：Docker 场景全部以 `--test-runtime` 内联执行，
  委托契约由 fixture E2E 与 systemd-nspawn smoke 覆盖；
- 卸载后事务清理与跨根历史事务语义由 fixture E2E 与单元测试覆盖，不在 Docker 场景
  中断言事务目录内容。
