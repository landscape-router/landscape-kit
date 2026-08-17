# 产品测试场景总目录

本文从产品行为出发组织 `landscape-kit` 需要验证的场景。详细场景按领域存放，一个领域
一个文件，稳定场景 ID 作为二级标题；准备步骤和完整断言继续由各专项文档说明。

覆盖状态只表示当前测试是否直接证明该行为，不表示功能是否已经实现：

- `已覆盖`：现有自动化脚本或测试包含直接断言；
- `部分覆盖`：只验证了部分结果、部分环境或较低层逻辑；
- `待补充`：没有能够直接证明该场景的测试；
- `低频 smoke`：只抽样验证外部系统兼容性，不作为普通发布门禁。

## 第一部分：核心功能测试

核心功能测试不依赖真实 systemd，是功能正确性和普通发布的主要判断依据。测试可以使用
fake systemctl 隔离外部 service manager，但资产发布与下载、文件系统变更、事务、
初始化、fixture 进程启停、端口和 API 检查均真实执行。

| 领域 | 场景 ID | 文档 |
| --- | --- | --- |
| lkit 自发布与引导安装 | `LKR-01` 至 `LKR-04` | [lkit-release.md](functional/lkit-release.md) |
| Release 发布与仓库 | `PUB-01` 至 `PUB-08` | [publish.md](functional/publish.md) |
| Ratatui 管理控制台 | `UI-01` 至 `UI-15` | [console.md](functional/console.md) |
| 命令行本地化 | `I18N-01` 至 `I18N-07` | [i18n.md](functional/i18n.md) |
| 首次安装 | `INS-01` 至 `INS-18` | [install.md](functional/install.md) |
| 手工部署迁移 | `MIG-01` 至 `MIG-05` | [migrate.md](functional/migrate.md) |
| 版本更新 | `UP-01` 至 `UP-09` | [update.md](functional/update.md) |
| 版本升级与切换 | `SW-01` 至 `SW-11` | [switch.md](functional/switch.md) |
| 备份与恢复 | `BKP-01` 至 `BKP-12`、`RST-01` 至 `RST-14` | [backup-and-restore.md](functional/backup-and-restore.md) |
| 卸载 | `UNI-01` 至 `UNI-14` | [uninstall.md](functional/uninstall.md) |
| 重新初始化 | `REI-01` 至 `REI-10` | [reinit.md](functional/reinit.md) |
| 自动备份与回滚 | `RB-01` 至 `RB-07` | [rollback.md](functional/rollback.md) |
| 修复 | `REP-01` 至 `REP-06` | [repair.md](functional/repair.md) |
| Reconcile 与事务 | `REC-01` 至 `REC-05`、`TX-01` 至 `TX-04` | [reconcile-and-transactions.md](functional/reconcile-and-transactions.md) |
| 安全与环境检查 | `SEC-01` 至 `SEC-03`、`ENV-01` 至 `ENV-03` | [security-and-environment.md](functional/security-and-environment.md) |
| 网络接管 | `NET-01` 至 `NET-11` | [network-takeover.md](functional/network-takeover.md) |
| 宿主网络适配 | `HNET-01` 至 `HNET-08` | [hostnet.md](functional/hostnet.md) |
| 主机换源 | `MIR-01` 至 `MIR-07` | [mirror.md](functional/mirror.md) |
| 常用软件安装 | `SFT-01` 至 `SFT-06` | [software.md](functional/software.md) |
| lkit 自身生命周期 | `SS-01` 至 `SS-09` | [self.md](functional/self.md) |
| daemon 自动恢复 | `DAE-01` 至 `DAE-04` | [daemon.md](functional/daemon.md) |
| Landscape Terrain 防失联通道 | `FLR-01` 至 `FLR-21` | [flare 文档](../../flare/scenarios.md) |

## 第二部分：systemd 兼容性 Smoke

systemd smoke 只验证 fake systemctl 无法证明的真实 manager 契约，不为每个业务场景
建立真实 systemd 副本，也不参与核心功能覆盖率。

| 领域 | 场景 ID | 文档 |
| --- | --- | --- |
| 真实 manager 与网络兼容性 | `SYS-01` 至 `SYS-06` | [systemd-smoke.md](systemd-smoke.md) |

## 当前优先缺口

优先补充能够改变发布判断的场景：

1. [`SS-05`](functional/self.md#ss-05) 至 `SS-08`：`lkit self upgrade` 全链路
   （下载校验→原子替换→daemon restart、同版本返回 `0`、失败保留原二进制、
   daemon 未注册仅更新 CLI），当前无任何测试（需给 `self upgrade` 增加
   GitHub URL 测试注入点）；
2. [`DAE-04`](functional/daemon.md#dae-04)：daemon 侧 switch 失败回滚完整现场
   （activating 阶段 + `.lkb` 配置级回滚）；
3. [`MIG-05`](functional/migrate.md#mig-05)：static.zip 缺失本地打包回退
   （`fetch_static_zip` 下载成功/失败两分支零测试）；
4. [`TX-03`](functional/reconcile-and-transactions.md#tx-03) 的 migrate/repair 恢复档、
   [`SEC-03`](functional/security-and-environment.md#sec-03) 的通用退出码 `6`、
   [`ENV-02`](functional/security-and-environment.md#env-02) 的命令级阻断级别、
   [`REC-02`](functional/reconcile-and-transactions.md#rec-02) 无
   `--accept-service-change` 时的交互确认路径、[`REI-08`](functional/reinit.md#rei-08)
   回滚失败（`6`）分支。

**委托端到端链路**（daemon worker：CLI 写请求 → daemon 认领 → 子进程执行 → 结果回收）
由 systemd-nspawn 兼容性 smoke 在真实 systemd 下覆盖：委托提交与结果回收、
前端断开后 daemon 独立完成、cancel 文件驱动的取消 + daemon 恢复（uninstall
无下载阶段，前端 Ctrl+C 按"仅 Downloading 可取消"契约被忽略）、daemon 未运行
拒绝、`LKIT_LANG` 转发（[SYS-03](systemd-smoke.md#sys-03)，仅 CI/手动运行）。
`delegate()` 请求文件生命周期与 executor 的 SIGTERM→SIGKILL 兜底仍无直接测试。

发布流程性 smoke（`PUB-08` 生产 RustFS 真实发布后安装、
`LKR-01`/`LKR-04` 首次真实 Release 与公开安装）依赖发布环境，维持低频标注。

恢复类失败路径（RB-06、REP-04、REP-05、RST-03/05/08~12）已由 Rust workflow 故障注入与
Docker E2E 直接覆盖；`RB-07`（回滚后下次调用按事务 phase 幂等恢复的完整 CLI 故障现场）
已由 Docker E2E S8/S12 覆盖。

### 文档状态维护说明

各场景文件的 `状态` 取值只有四档：`已覆盖`/`部分覆盖`/`待补充`/`低频 smoke`。
新增或改名测试后必须同步更新对应场景的 `状态` 与 `证据` 行，避免文档滞后产生
虚假缺口。

现有 [发布、安装与成功切换](lifecycle.md)、[失败切换与自动回滚](rollback.md)和
[扩展 Docker 功能 E2E](extended.md)继续保存已落地场景的详细执行步骤。数据库级完整恢复
和空目录灾难重建不属于本版 backup/restore 范围；卸载见
[uninstall.md](functional/uninstall.md)。
