# 里程碑与待确认事项

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 里程碑说明

M1-M3 按实现顺序推进，每个里程碑产出可运行的功能增量。M4 是 **验收检查清单**，确认 M1-M3 全部功能的 CLI 入口均已完整实现（非交互参数化、错误码、输出格式）。

## 3. 首版里程碑

状态：[done] 已完成 | [wip] 进行中 | [ ] 未开始

### M1：CLI 骨架与通用启动器 [done]

- `clap` 命令骨架，所有子命令可注册
- `lkit`（无参数）通用启动器主菜单
- `lkit status` / `lkit logs` / `lkit service {start|stop|restart}`（功能实现）
- `lkit diagnose` 基础能力（含 doctor 原检查项）
- 本机入口可在无网络场景下使用

### M2：安装与初始化 [done]

- `lkit install` 命令（无参数进入引导式 Wizard，`--init-file` 非交互安装）[done]
- release source 解析 [done] 已由 M2.5 的多源架构覆盖（GitHub / HTTP / local / S3）
- binary + `static.zip` 获取与校验 [done] 已在 `lkit mirror sync` + `verify` 中实现
- systemd 安装流程 [done] InstallExecutor + systemd unit 模板
- 自动初始化与首次启动检查 [done] landscape_init.toml 生成 + 权限保护
- 引导式网络配置 [done] 7 步可回退 Wizard（WAN/LAN 选择、IP 模式、网关、服务、源、确认）

### M2.5：多源下载与镜像工具 [done]

- `ReleaseSource` trait 与 `SourceResolver`（多源并发探测）
- `ArtifactDownloader` trait（文件级并行 + 可选单文件分块）
- `release-manifest.json` schema 实现
- `lkit-mirror` lib crate + `lkit mirror` 内置子命令
- Target 抽象：`LocalTarget` + `S3Target`（R2/MinIO/S3）
- Source 抽象：`GithubSource` + `HttpMirrorSource` + `LocalSource` + `S3Source`
- `lkit mirror sync`：范围控制（`--tag` / `--latest N` / `--since` / `--all`）+ 增量同步
- `lkit mirror serve` / `verify` / `list`
- `--source` 参数（github/http/local/s3）支持 mirror-to-mirror
- 内置默认源：R2 HTTP 优先 + GitHub fallback
- 镜像目录规范文档
- `lkit install` 切换到多源并发探测下载 [done] PR#5 已接入 SourceResolver
- 详细设计见 [09-release-source.md](./09-release-source.md)

### M3：备份、恢复与更新回滚 [ ]

- `config export`
- `backup create/list/restore/delete`
- 手动备份入口
- 备份包格式
- frozen backup index
- 升级前自动备份
- 升级失败回滚
- 健康检查

### M4：验收检查清单

- [x] `lkit install --init-file` 非交互安装完整可用
- [x] `lkit install --source` / `--version` 参数化安装完整可用
- [x] `lkit status` 输出格式稳定（表格/JSON）
- [ ] `lkit backup create/list/restore/delete` 完整可用
- [ ] `lkit upgrade check/apply` + `lkit rollback list/apply` 完整可用
- [x] `lkit diagnose` 检查项完整、输出格式稳定
- [x] `lkit mirror sync --target local` 完整可用
- [x] `lkit mirror sync --target s3` 完整可用（R2/MinIO）
- [x] `lkit mirror verify` 校验准确性确认
- [x] `lkit mirror list` 输出格式稳定
- [x] `lkit mirror serve` HTTP 服务可正常提供制品下载
- [ ] 所有命令返回一致的退出码
- [ ] 非 TTY 下所有命令可无交互执行

## 4. 待确认事项与 V1 默认决定

以下事项在 V1 采取默认决定，可在后续版本调整：

| # | 事项 | V1 默认决定 |
|---|------|------------|
| 1 | 发行形态 | **单二进制**，包管理器留到 V2 |
| 2 | 非 TTY 下 `install` 的默认语义 | 强制要求 `--init-file` 或显式 `--non-interactive` |
| 3 | 已安装实例再次执行 `install` | 默认拒绝，提示使用 `--force`（会先创建自动备份点） |
| 4 | 版本解析策略 | 默认取 `latest`，管理器配置可固定默认版本 |
| 5 | systemd unit 参数形式 | `ExecStart` 显式传参（`--home`、`--web-root`） |
| 6 | 校验强度 | 强制要求 checksum；manifest 缺失时 fallback 到 `SHASUM256sum.txt` |
| 7 | `metric/` 策略 | 作为高级备份/恢复选项，不进入默认实例恢复面 |
| 8 | 管理器自身更新范围 | V1 只做 `check` 提示，`apply` 留到 V2 |

## 5. 最终设计结论

- 采用 **独立仓库 + 共享应用层 + 引导式 CLI 优先交付** 方案
- 首版运行在 **Landscape 所在主机**，重点解决本机安装、管理、离线救援
- 首版 **不做守护进程**，**不新增外部 API**；管理器通过本地系统操作与现有 Landscape API 完成工作
- `lkit` 无参数进入 **通用启动器**，是唯一交互入口；各子命令支持非交互直接调用
- release source 采用 **多源并发探测**，支持 GitHub / HTTP 镜像（含 R2）/ 本地路径；`lkit mirror` 内置子命令负责镜像管理
- V1 release 以 **Landscape binary + `static.zip`** 为核心制品集合，服务安装方式仅支持 **systemd**
- `landscape_init.toml` 仅用于 **初始化 / 重建**，不是默认实例恢复面的核心依赖
- 默认实例恢复面见 [04-backup-restore](./04-backup-restore.md) 权威定义
- `metric/` 可作为高级可选项；`logs/`、`geo_tmp/`、`landscape_api_token` 不纳入 V1 默认恢复面
- 建议由 Landscape 提供 `landscape_backup_index.json`，作为备份/恢复范围与语义的权威声明
- 升级采用 **事务式流程**：自动备份 -> 获取制品 -> 应用更新 -> health check -> 失败回滚
- CLI 首版必须把 **安装、手动备份、恢复、升级、回滚、运行管理、救援入口** 作为一等能力
