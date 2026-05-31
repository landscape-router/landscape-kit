# Landscape Kit 设计规格

## 文档信息

- 状态：Draft
- 仓库名：`landscape-kit`
- 二进制入口：`lkit`

## 文档目录

1. [概览与产品边界](./01-overview.md)
2. [功能范围与交互边界](./02-feature-scope.md)
3. [安装、初始化与生命周期](./03-lifecycle.md)
4. [备份、恢复、升级与回滚](./04-backup-restore.md)
5. [技术架构与代码结构](./05-architecture.md)
6. [里程碑与待确认事项](./06-roadmap.md)
7. [i18n 设计考量](./07-i18n.md)
8. [测试策略](./08-testing.md)
9. [Release Source 与镜像管理](./09-release-source.md)
10. [安装流程实现规格](./10-install-flow.md)

## 核心结论

- 独立仓库 + 共享应用层 + 引导式 CLI 优先交付
- 首版运行在 Landscape 所在主机，解决本机安装、管理、离线救援
- 不做守护进程、不新增外部 API
- `lkit` 无参数进入通用启动器，是所有交互操作的唯一入口；各子命令支持非交互直接调用
- release source 多源并发探测：`ReleaseSource` trait 统一抽象，支持 GitHub / HTTP 镜像（含 R2）/ 本地路径，自动选最快源
- `lkit-mirror` lib crate 负责镜像管理逻辑，`lkit mirror` 内置子命令（sync/serve/verify/list），上游零改动
- V1 仅支持 systemd
- `landscape_init.toml` 只用于初始化/重建，不做常驻配置
- 默认实例恢复面：[04-backup-restore](./04-backup-restore.md) 为权威定义：`landscape-webserver` + `static/` + `landscape_init.toml`（API 导出）
- 升级采用事务式流程：自动备份 → 获取制品 → 应用更新 → health check → 失败回滚
- `upgrade` 指版本升级操作，`self upgrade` 指管理器自身升级
