# 手工部署迁移

从非 lkit 安装格式（手工部署，如 `/root/.landscape-router`）迁移为 lkit 受管安装。
CLI 级 E2E 使用 landscape fixture 作为运行中的旧实例、fake systemctl 接管旧 unit，
见 [`migrate.rs`](../../../../lkit-cli/tests/install_fixture_e2e/migrate.rs)。

## MIG-01

**运行中的手工部署迁移（systemd 接管，完整 CLI）**

- 测试层：核心功能（E2E + 单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`migrates_manual_deployment_through_full_cli`（lkit-cli/tests/install_fixture_e2e/migrate.rs）
- 说明：fixture 实例运行中 → 迁移创建 `.lkb`（旧版本不升级）→ 停止旧 unit → 重建 release/data/current → 注册并启动新受管实例 → 完整健康检查后提交 complete 状态，旧目录不被修改。内联执行（`--test-runtime` 内联 runtime）。

## MIG-02

**systemd 旧 unit 识别与接管**

- 测试层：核心功能（E2E + 单元测试，fake systemctl）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`migrates_in_systemd_mode_with_legacy_unit_adoption`（lkit-cli/src/workflows/migrate/tests.rs）
- 说明：按 `ExecStart` 的 config 目录参数（`--config-dir` 或短形式 `-c`）匹配发现旧 unit → stop/disable（fake systemctl 按预置 `main.pid` 真实结束旧实例进程）→ 原件位于 `/etc/systemd/system` 时移入事务目录 → 新受管 unit 的 MainPID 指向迁移后的 release 二进制。单元测试 fixture 与旧 unit 都用真实部署常用的短形式 `-c`/`-w` 书写。

## MIG-03

**激活失败自动回滚并恢复旧 unit**

- 测试层：核心功能（E2E + 单元测试，fake systemctl）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md#失败与恢复)、`migrate_rolls_back_and_restores_legacy_unit_on_activation_failure`（lkit-cli/tests/install_fixture_e2e/migrate.rs）、`systemd_mode_rolls_back_and_restores_legacy_unit_on_activation_failure`（lkit-cli/src/workflows/migrate/tests.rs）
- 说明：新实例启动即退出 → 自动回滚：注销新 unit、旧 unit 文件放回原位、清理新根、事务 `rolled_back`，CLI 退出码 `5`。

## MIG-04

**非交互确认与源目录校验**

- 测试层：核心功能（单元测试）
- 状态：`已覆盖`
- 证据：`migrate_requires_yes_in_non_interactive`、`validates_source_directories`（lkit-cli/src/workflows/migrate/tests.rs）
- 说明：非交互缺 `--yes` 时不创建事务；源目录必须含特征文件、必须是真实目录、不能是受管安装的 data。

## MIG-05

**迁移备份的 static.zip 现场打包**

- 测试层：核心功能（单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`pack_static_zip`（lkit-cli/src/release/repository/archive.rs）
- 缺口：打包自校验失败（static 目录含符号链接等非法条目）在 migrate 路径无直接
  断言（备份通用路径 `backup/lkb/write.rs` 的 `rejects_symlinks_in_source_tree` 覆盖）。
- 说明：迁移备份的 `static.zip` 由 `create_backup` 从旧部署的 `static/` 现场打包
  （与备份内 `static/` 树同源同刻），不再从发布仓库下载或复用本地 `static.zip`。

## MIG-08

**无 config 参数的真实部署（裸二进制 + cgroup 反查）**

- 测试层：核心功能（单元测试 + 真实主机验证）
- 状态：`部分覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`judges_external_landscape_without_config_args`（lkit-cli/src/service/process.rs）、`extracts_unit_name_from_cgroup`（lkit-cli/src/service/systemd.rs）
- 缺口：cgroup 反查完整链路只在真实 systemd 主机上可触发（单元测试的 fixture
  进程 cgroup 不含 `.service`），需在真实部署上验证；
- 说明：旧实例 cmdline 完全不带 config 参数（如 `ExecStart=/root/landscape-webserver`）
  时，实例识别回退到可执行文件身份（位于源目录内或文件名含
  `landscape-webserver`），导出 API 校验（`--from/landscape_api_token`）为最终防线；
  旧 unit 发现按实例 cgroup 反查所属 unit（已安装才接管），避免误停无关 unit。

## MIG-09

**旧安装器普通文件 unit 的接管（所有权冲突预清）**

- 测试层：核心功能（单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`preempts_a_plain_file_legacy_unit_at_the_managed_path`、
  `migrates_a_plain_file_legacy_unit_at_the_managed_path`（lkit-cli/src/workflows/migrate/tests.rs）
- 说明：旧安装器把 unit 以普通文件直接写入受管路径 `/etc/systemd/system/landscape-router.service`
  时，systemd 注册的所有权保护会拒绝覆盖；实例识别已确认该 unit 属于旧部署
  （`ExecStart` 匹配源目录）后，切换阶段先 `stop` + `disable` 并把文件移入事务
  目录，再按「未注册」接管，成功后旧文件保留在事务目录、回滚放回原位。符号
  链接接管形态、其他 unit 名、无关文件不预清，仍由所有权保护阻断。

## MIG-10

**切换期间取消：Ctrl+C 回滚恢复旧实例**

- 测试层：核心功能（E2E + 单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`switch_cancellation_rolls_back_with_the_cancelled_outcome`
  （lkit-cli/src/workflows/migrate/tests.rs）、`cancelling_the_delegated_switch_restores_the_old_instance`
  （lkit-cli/tests/install_fixture_e2e/migrate.rs）
- 说明：迁移切换完全由事务保护，允许取消。委托路径下 Ctrl+C → 前台写 cancel
  文件 → daemon 对 worker 进程组 SIGTERM → worker 在安全点（阶段边界、健康检查
  等待期间）感知后自动回滚（恢复旧 unit 与 enabled/active 状态、清理新根），
  前台等待收尾后以 `130` 退出并输出"迁移已取消"。E2E 对前台发 SIGINT 验证
  退出码 130、旧 unit 恢复、旧实例重新 active、事务终态 `rolled_back`；内联路径
  由单元测试用递增检查次数的中断闭包验证产出 `Cancelled` 结果。

## MIG-06

**导出 API 支持检查（旧部署不支持时明确失败）**

- 测试层：核心功能（单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`classifies_missing_export_api_as_unsupported`（lkit-cli/src/backup/export.rs）
- 说明：`GET /api/v1/system/config/export` 返回 `404` 时迁移报 `ExportUnsupported`
  （部署的 Landscape 不提供 config export API，需先升级旧部署），与 `500` 等
  服务端故障（`ExportFailed`）区分；检查发生在创建事务之前，失败不留任何现场。

## MIG-07

**两阶段迁移：前台前置检查 + worker 切换委托**

- 测试层：核心功能（E2E + 单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`prepared_migration_resumes_the_switch_phase_in_the_worker`、`resume_rejects_an_unknown_prepared_transaction`（lkit-cli/src/workflows/migrate/tests.rs）、`migrate_delegates_follows_runtime_and_euid`（lkit-cli/src/daemon_worker/mod.rs）、`migrates_manual_deployment_through_daemon_delegation`（lkit-cli/tests/install_fixture_e2e/migrate.rs）
- 说明：root 下 `lkit migrate` 在前台进程直接执行前置检查（源目录、实例识别、
  export API 检查、迁移 `.lkb`、计划确认），事务标记 `prepared` 后以内部参数
  `--resume <事务 id>` 委托 daemon worker 只执行切换阶段；worker 要求事务 id
  精确匹配、`prepared` 阶段且已记录备份，否则拒绝。非 root / 测试 runtime 整条
  流程内联（`migrate_version`）。前台阶段 Ctrl+C 中止且不触碰旧实例。
  E2E 直接覆盖真实委托链路：测试 spawn 常驻 daemon（`execution=daemon` runtime），
  前台前置检查 → daemon 认领请求 → 子进程 `--resume` 执行切换 → 结果回收。
