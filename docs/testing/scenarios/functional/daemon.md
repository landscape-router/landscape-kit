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
- 证据：[完整 CLI E2E](../../../../lkit-cli/tests/install_fixture_e2e/daemon.rs)
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

## DAE-05

**daemon 启动时尽力拉起所有物理以太网卡，保证 flare L2 防失联通道可用**

- 测试层：Rust 单元测试（`network::ifup`，临时 sysfs 目录 + 假 `ip` 命令）
- 状态：`已覆盖（单元）`
- 证据：[flare 协议](../../flare/protocol.md#部署形态)、`lkit-cli/src/network/ifup.rs`
- 缺口：真实网卡上的行为（对 DOWN 的物理网卡执行 `ip link set dev <name> up`）
  未纳入 fixture E2E
- 说明：网卡 DOWN 时 flare 服务端即使运行也无法收发帧，恢复通道失效。daemon
  启动、托管 flare 之前枚举 `/sys/class/net`，对处于 DOWN 状态的物理以太网卡
  执行 `ip link set dev <name> up`；跳过 loopback、无线与虚拟设备，无 MAC
  （`address` 缺失或为空）的网卡也跳过；已 UP 的网卡不发命令；任何失败只
  记录日志，不阻断 daemon 启动。

## 委托端到端链路（worker）覆盖现状

`daemon_worker` 的委托请求链路（CLI 写请求 → daemon 认领 → 子进程执行 → 结果回收）
单测只有 5 个薄用例（委托清单、worker flag 注入、凭据文件权限、完成消息）；fixture
E2E 全部以 `--test-runtime` 内联执行，从不走真实委托。真实形态的端到端验证在
[systemd-nspawn 兼容性 smoke](../systemd-smoke.md#sys-03)（`scripts/test-nspawn-systemd.sh`，
仅 CI/手动运行）：编译好的 lkit 进入真实 systemd 容器，`self install` 由 systemd
托管 daemon，CLI 以真实委托执行卸载——覆盖提交与结果回收（S-1）、前端断开后
daemon 独立完成（S-2）、cancel 文件驱动的取消 + daemon 恢复（S-3，uninstall
无下载阶段，前端 Ctrl+C 按"仅 Downloading 可取消"契约被忽略）、daemon 未运行
拒绝（S-4）、`LKIT_LANG` 转发（S-5）。

`delegate()` 请求文件生命周期与 executor 的 SIGTERM→SIGKILL 兜底（`CANCEL_GRACE_POLLS`
超时后强杀）仍无直接测试。
