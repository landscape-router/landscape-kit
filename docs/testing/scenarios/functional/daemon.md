# daemon 自动恢复场景

## DAE-01

**CLI 中断后 daemon 自动恢复遗留事务（激活中途消失 → 失败清理）**

- 测试层：Fixture E2E（`install_fixture_e2e::daemon`）
- 状态：`已覆盖`
- 证据：[事务与中断恢复](../../deployment/transactions-and-recovery.md#daemon-自动恢复)
- 说明：构造 `activating` 阶段的 install 事务现场，启动 daemon 后断言事务标记
  `failed`、目标 release/current/初始化文件被清理；恢复后 daemon 继续运行，
  SIGTERM 时清理 pidfile 干净退出。

## DAE-02

**daemon 尊重安装锁：CLI 持有锁期间不触碰事务，释放后恢复**

- 测试层：Fixture E2E（`install_fixture_e2e::daemon`）
- 状态：`已覆盖`
- 说明：测试进程持锁期间断言事务保持原阶段，释放锁后 daemon 下一周期完成恢复；
  验证并发安全边界。

## DAE-03

**daemon 对网络接管待确认阶段不代替用户确认**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e/daemon.rs)
- 说明：构造带 `network_takeover` 的 `awaiting_network_confirmation` 事务并启动
  daemon，多个周期后事务保持原阶段、安装现场不被触碰、daemon 存活；
  恢复循环显式跳过 `awaiting_network_confirmation`/`finalizing`/`rolling_back`。

## DAE-04

**daemon 周期恢复覆盖 systemd 失败切换（`.lkb` 配置级回滚）**

- 测试层：Rust workflow（`recover_switch`）、Fixture E2E 待补充
- 状态：`部分覆盖`
- 说明：`recover_interrupted` 的 switch 回滚语义已有 Rust 覆盖（仅 preparing 分支）；
  daemon 作为其触发者与 CLI 复用同一代码路径，daemon 侧的完整失败切换现场
  （activating 阶段 + `.lkb` 配置级回滚）待补充。

## 委托端到端链路（worker）覆盖现状

`daemon_worker` 的委托请求链路（CLI 写请求 → daemon 认领 → 子进程执行 → 结果回收）
当前只有 5 个薄单测（委托清单、worker flag 注入、凭据文件权限、完成消息），
核心流程无直接测试：

- `delegate()` 请求文件生命周期、executor 认领与子进程管理（setpgid/O_NOCTTY/
  输出转发/结果 JSON 原子提交）、wait 轮询/取消/超时均无单测；
- fixture E2E 全部以 `--test-runtime` 内联执行，**从不走真实委托**；
- 委托端到端全仓库只有 systemd-nspawn SYS-03 一处覆盖，且只测提交路径。

计划（仅 CI 运行，不支持本地手动跑）：在 `install_fixture_e2e` 增加真实委托端到端
测试——CLI 不带 `--test-runtime`（root + 无 test_runtime 即走真实委托）、spawn 的
测试 daemon、真实 `/run/lkit/operations` 路径，覆盖：委托提交、取消（SIGTERM→SIGKILL
→130）、前端断开后 daemon 子进程组继续完成、`LKIT_LANG` 环境转发、daemon 未运行
报错。该测试需要 root，仅由 CI（`LKIT_E2E=1`）运行。
