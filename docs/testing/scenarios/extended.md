# 扩展 Docker 功能 E2E 场景

## 目标与约束

Docker 功能 E2E 在 install、成功 switch 和健康失败回滚的基础上覆盖以下扩展场景。
约束与基础场景一致：

- 被测 `lkit` 使用 `test-support` 构建，通过 `--test-runtime` 显式选择
  `preflight: skip`、`execution: inline`；
- 使用 fake systemctl 和隔离的 unit/resolv.conf 路径，但仍绑定固定端口
  `53/6300/6443` 并启动真实 fixture 进程；
- fixture 只通过版本化的 `static/lkit-fixture.json` 切换声明式场景；
- 每个新 release 使用 `lkit-fixture-release --stamp-version` 获得唯一二进制摘要；
  fixture 只编译一次。

本文档记录 `run-scenarios.sh` 当前执行的场景和断言。

## 场景总览

| 编号 | 场景 | fixture 场景 | 前置状态 | 命令 | 核心验证点 |
| --- | --- | --- | --- | --- | --- |
| S1 | repair 全流程 | healthy | 已安装 1.0.0 | `lkit repair binary/static` | 从仓库恢复二进制与静态页，SHA 一致 |
| S2 | 导出失败回滚 | export_error | 独立安装根上 4.0.0 运行中 | `lkit switch 4.1.0` | 备份阶段失败的不同回滚路径 |
| S3a | 启动即退 | start_exit | 已运行 2.0.0 | `lkit switch 4.1.0` | 进程启动后立即退出，失败清理 |
| S3b | 稳定期退出 | exit_during_stability | 已运行 2.0.0 | `lkit switch 4.2.0` | 就绪后退出，稳定观察失败 |
| S3c | 慢启动超时 | delayed_ready | 已运行 2.0.0 | `lkit switch 4.3.0` | 启动轮询超时 |
| S4 | 停止服务后切换 | healthy | 2.0.0 停止 | `systemctl stop` + `lkit switch 5.0.0 [--allow-no-backup]` | 停止服务默认拒绝；显式允许后无备份切换 |
| S6 | 服务管理器迁移 | healthy | `--service-manager none` 安装 6.0.0 | `lkit service-manager systemd` | none → systemd 迁移 |
| S7 | latest 通道安装 | healthy | 服务已停止、端口空闲 | `lkit install`（无版本） | 解析 stable 并安装 |
| S8 | 中断事务恢复 | healthy | 手工制造 preparing 现场 | `lkit switch 6.0.0` | 确定性恢复未完成事务 |
| S9 | reconcile | healthy | 外部修改/破坏受管元数据 | `lkit reconcile` | 校验并接受受管元数据变化 |

版本规划（避免与现有 1.0.0/2.0.0/3.0.0 冲突）：

```text
4.0.0 export_error（S2 独立安装根）
4.1.0 start_exit（S3a；S2 的失败切换目标）
4.2.0 exit_during_stability
4.3.0 delayed_ready（ready_delay_ms = 10000，超过 4 秒测试启动超时）
5.0.0 healthy（S4/S7 共用）
6.0.0 healthy（S6/S8 共用）
```

## S1 repair 全流程

前置：`1.0.0` 已安装并运行（复用现有 `assert_service_identity`）。

1. 篡改二进制：复制 `releases/1.0.0/landscape-webserver` 到临时文件，追加字节后原子
   替换原路径，使 SHA 与 `state.assets.webserver.sha256` 不一致，避免直接写入正在执行的
   ELF；
2. 执行 `lkit repair binary --install-dir <root>`，从仓库重新下载并校验：
   - 退出码 `0`；
   - `sha256sum releases/1.0.0/landscape-webserver` 与状态记录一致；
   - 修复创建版本为 `1.0.0` 的 `.lkb`（`assert_backup_metadata`）；
   - `assert_service_identity` 通过（服务身份与 HTTPS 探针）；
3. 删除 `current/static/` 下的页面文件，执行 `lkit repair static`：
   - `current/static/index.html` 与 `current/static/lkit-fixture.json` 恢复；
   - 服务仍运行，事务 phase 为 `committed`。

`--repair-binary` 走 `allow_sha_drift` 放行路径（preflight 端口占用豁免）。Docker 场景
验证该豁免与 service-manager 协议的组合；nspawn 只低频抽样验证真实 systemd 兼容性，
不重复该功能场景。

## S2 导出失败回滚（export_error）

配置导出 API 由**运行中**的服务提供，`.lkb` 备份的内容来自该次导出。因此
export 失败（发生在备份创建之前）只能在 export_error 版本**正在运行**时被触发；
切换到 export_error 版本本身会成功（fixture 可正常启动，`/api/docs` 返回 `200`）。
该场景使用独立的安装根，避免污染主安装根的后续切换：

1. 将当前持有全局 unit 的安装根迁移到 none，释放固定端口和注册链接，再安装
   `4.0.0`（export_error）到独立根：
   ```sh
   lkit install --version 4.0.0 --install-dir <root-export> --service-manager systemd
   ```
2. 尝试切换到尚未安装的 `4.1.0`：
   ```sh
   lkit switch --version 4.1.0 --install-dir <root-export>
   ```
3. 必须断言：
   - 命令返回失败退出码；
   - active version 与 `current` 仍为 `4.0.0`；
   - 最新 switch 事务 phase 为 `failed`（导出失败发生在停止服务之前，没有回滚发生）；
   - 服务仍为 `4.0.0` 且健康；
   - 不产生新的 `.lkb`。

与 `health_error`（S3 之后、激活阶段失败）不同，export 失败发生在**备份创建之前**，
事务在 `preparing` 阶段直接标记 `failed`。失败前 `build_release` 已把目标 release 目录
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

配置导出 API 由运行中的服务提供，服务停止后无法导出配置快照，因此 v1 的
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

## S6 服务管理器迁移（none → systemd）

none → systemd 迁移需要用户通过 `/dev/tty` 确认"外部实例已停止"，并确认固定端口
已释放。非交互 Docker runner 没有控制终端，场景通过 `script`（util-linux）提供 pty
并向其输入一行 `yes`：

1. 将当前持有全局 unit 的安装根迁移到 none，释放固定端口和注册链接，再安装
   `6.0.0` 到独立根：
   ```sh
   lkit install --version 6.0.0 --install-dir <root-migrate> --service-manager none
   ```
   - state 中 `service.manager == none`，初始化状态为 `pending`；
   - 无 systemd unit 注册（`systemctl is-enabled` 失败）；
2. `lkit service-manager systemd --install-dir <root-migrate>`（经 pty 确认）：
   - unit 注册、enable、start；
   - state `service.manager == systemd`，初始化完成；
   - `assert_service_identity` 通过。

迁移路径是 `RequestMode::ServiceManager`。Docker 场景覆盖
文件模式 → systemd 协议模式的完整状态转换；nspawn 只对真实 manager 转换做低频
兼容性 smoke，不作为该业务场景的第二套必需验收。

## S7 latest 通道安装

固定端口被现有服务占用，因此需要先释放端口：

1. `lkit service-manager none --install-dir <current-root>`，释放全局 unit；
2. 使用新的 `--install-dir` 执行 `lkit install --repository <base>`（不带版本）：
   - 解析 `channels/stable.json` 为 `5.0.0`（此时 6.0.0 尚未发布）；
   - 安装成功并注册新服务；
   - state `active_version == 5.0.0`，仓库来源为 HTTP。

## S8 中断事务恢复

以确定性方式模拟 kill -9 中断（不真正 kill）：

1. 在 S7 的安装根上（当前 `5.0.0` 运行中）手工写入一个 switch 事务文件
   （phase `preparing`，from `5.0.0` → target `6.0.0`），并制造现场：
   目标 release 目录 `releases/6.0.0/` 半成品（只含部分文件）、`current` 未动；
2. 发布 `6.0.0` 后执行 `lkit switch --version 6.0.0 --install-dir <root-latest>`，
   触发 `recover_interrupted`；
3. 断言：
   - 命令正常完成（退出码 `0`），无残留未完成事务；
   - 手工制造的事务 phase 为 `failed`（preparing 阶段恢复：清理半成品目标
     release 目录并标记失败），新 switch 事务 phase 为 `committed`；
   - 最终 active version 与 `current` 为 `6.0.0`，服务身份与 state 一致；
   - 新切换创建版本为 `5.0.0` 的 `.lkb`。

事务 JSON 格式以 `deployment/transaction` 模块的校验规则为准，场景脚本直接构造
（`schema_version`、`log_path`、`canonical_install_root` 等字段必须合法）。

恢复逻辑与目标版本无关（preparing 阶段不读取旧状态）。该场景使用
`5.0.0 → 6.0.0`，避免与主安装根上 S4 的 `5.0.0` 切换竞争同一版本。

## S9 reconcile

`lkit reconcile` 的语义是"检查活动版本并协调受管元数据的外部变化"，
**不会**从缺失状态或损坏链接重建状态。`load_state` 对缺失 `current` 链接或
`current` 漂移直接失败，缺失 `state/install-state.json` 时按首次安装处理并拒绝。
因此场景按以下语义断言：

1. 在初始化已完成的安装中，外部向 `data/landscape_init.toml` 追加标记，执行
   `lkit reconcile --install-dir <root>`：
   - 退出码 `0`，文件字节不变，state 不包含初始化文件摘要；
2. 再次执行 `lkit reconcile`（无变化、无需确认）：退出码 `0`，状态文件有效；
3. 删除 `state/install-state.json`，执行 `lkit reconcile`：
   - 命令拒绝（非零退出码），不重建状态文件；恢复状态文件；
4. 破坏 `current` 链接（指向 `releases/2.0.0`），执行 `lkit reconcile`：
   - 命令拒绝激活漂移（非零退出码）；恢复 `current` 后再次执行通过，
     服务身份与 state 一致。

## 明确不做

- SQLite 数据库级备份、空目录灾难重建和数据库内容恢复；当前 `.lkb` 仍是 minimal 配置级备份；
- 真实 kill -9 崩溃（用 S8 的确定性现场模拟替代）；
- QEMU 与完整宿主 preflight；systemd-nspawn 另有低频兼容性 smoke test。
