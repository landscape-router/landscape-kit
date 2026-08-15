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

- 测试层：待补充
- 状态：`待补充`
- 说明：恢复循环显式跳过 `awaiting_network_confirmation`/`finalizing`/
  `rolling_back` 阶段；缺少直接断言场景。

## DAE-04

**daemon 周期恢复覆盖 systemd 失败切换（`.lkb` 配置级回滚）**

- 测试层：Rust workflow（`recover_switch`）、Fixture E2E 待补充
- 状态：`部分覆盖`
- 说明：`recover_interrupted` 的 switch 回滚语义已有 Rust 覆盖；daemon 作为其
  触发者与 CLI 复用同一代码路径，daemon 侧的完整失败切换现场待补充。
