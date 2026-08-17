# 手工部署迁移

从非 lkit 安装格式（手工部署，如 `/root/.landscape-router`）迁移为 lkit 受管安装。
CLI 级 E2E 使用 landscape fixture 作为运行中的旧实例、fake systemctl 接管旧 unit，
见 [`migrate.rs`](../../../../lkit-cli/tests/install_fixture_e2e/migrate.rs)。

## MIG-01

**运行中的手工部署迁移（systemd 接管，完整 CLI）**

- 测试层：核心功能（E2E + 单元测试）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`migrates_manual_deployment_through_full_cli`（lkit-cli/tests/install_fixture_e2e/migrate.rs）
- 说明：fixture 实例运行中 → 迁移创建 `.lkb`（旧版本不升级）→ 停止旧 unit → 重建 release/data/current → 注册并启动新受管实例 → 完整健康检查后提交 complete 状态，旧目录不被修改。

## MIG-02

**systemd 旧 unit 识别与接管**

- 测试层：核心功能（E2E + 单元测试，fake systemctl）
- 状态：`已覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`migrates_in_systemd_mode_with_legacy_unit_adoption`（lkit-cli/src/workflows/migrate/tests.rs）
- 说明：按 `ExecStart --config-dir` 匹配发现旧 unit → stop/disable（fake systemctl 按预置 `main.pid` 真实结束旧实例进程）→ 原件位于 `/etc/systemd/system` 时移入事务目录 → 新受管 unit 的 MainPID 指向迁移后的 release 二进制。

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

**static.zip 缺失时的本地打包回退**

- 测试层：核心功能（单元测试辅助路径，发布仓库不可达时触发）
- 状态：`部分覆盖`
- 证据：[migrate 命令](../../commands/migrate.md)、`pack_static_zip`（lkit-cli/src/workflows/migrate/mod.rs）
- 缺口：`fetch_static_zip` 的下载成功/失败两分支零测试；`pack_static_zip` 只在测试
  setup 中被动执行，无回退断言（注入仓库不可达即可触发回退）。
- 说明：本地缺 `static.zip` 时先尝试从发布仓库下载该版本，失败后从 `static/` 现场打包并按仓库解包规则自校验。
