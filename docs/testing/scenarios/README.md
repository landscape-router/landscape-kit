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
| Release 发布与仓库 | `PUB-01` 至 `PUB-08` | [publish.md](functional/publish.md) |
| 首次安装 | `INS-01` 至 `INS-12` | [install.md](functional/install.md) |
| 版本升级、降级与切换 | `SW-01` 至 `SW-10` | [switch.md](functional/switch.md) |
| 备份与回滚 | `RB-01` 至 `RB-07` | [rollback.md](functional/rollback.md) |
| 修复 | `REP-01` 至 `REP-06` | [repair.md](functional/repair.md) |
| Service Manager 迁移 | `SM-01` 至 `SM-07` | [service-manager.md](functional/service-manager.md) |
| Reconcile 与事务 | `REC-01` 至 `REC-05`、`TX-01` 至 `TX-04` | [reconcile-and-transactions.md](functional/reconcile-and-transactions.md) |
| 安全与环境检查 | `SEC-01` 至 `SEC-03`、`ENV-01` 至 `ENV-02` | [security-and-environment.md](functional/security-and-environment.md) |

## 第二部分：systemd 兼容性 Smoke

systemd smoke 只验证 fake systemctl 无法证明的真实 manager 契约，不为每个业务场景
建立真实 systemd 副本，也不参与核心功能覆盖率。

| 领域 | 场景 ID | 文档 |
| --- | --- | --- |
| 真实 manager 兼容性 | `SYS-01` 至 `SYS-04` | [systemd-smoke.md](systemd-smoke.md) |

## 当前优先缺口

优先补充能够改变发布判断的场景：

1. [`PUB-08`](functional/publish.md#pub-08)：生产 RustFS 上的真实 Release 发布后安装 smoke；
2. [`RB-06`](functional/rollback.md#rb-06)：自动回滚自身失败及退出码 `6`；
3. [`SW-02`](functional/switch.md#sw-02)：显式降级到历史版本；
4. [`REP-04`](functional/repair.md#rep-04)、[`REP-05`](functional/repair.md#rep-05)：repair 失败后的回滚与回滚失败；
5. [`SM-07`](functional/service-manager.md#sm-07)：service-manager 迁移失败恢复。

现有 [发布、安装与成功切换](lifecycle.md)、[失败切换与自动回滚](rollback.md)和
[扩展 Docker 功能 E2E](extended.md)继续保存已落地场景的详细执行步骤。公开
`lkit backup`、`lkit restore`、数据库完整恢复和卸载当前尚未实现，不进入本目录。
