# 功能范围与交互边界

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 术语约定

- `upgrade`：版本升级操作，对 Landscape 为 `lkit upgrade`，对管理器自身为 `lkit self upgrade`
- `rollback`：升级失败后的回滚操作

## 3. 首版功能范围

### 3.1 CLI 命令清单

首版 CLI 统一通过 `lkit` 暴露。`lkit` 命令默认面向 Landscape 操作，`lkit self` 面向管理器自身。

```
lkit                                    # 通用启动器（无参数进入主菜单）
lkit install --init-file <file>         # 非交互安装（--init-file 必填）
lkit install --init-file <file> --source <source> --version <version>
                                        # 额外指定 release 来源/版本
lkit status [--json]                    # Landscape 运行状态 + systemd 服务状态
lkit service {start|stop|restart}       # systemd 服务控制
lkit logs                               # Landscape 日志
lkit backup create                      # 创建备份点
lkit backup list [--json]               # 列出备份点
lkit backup restore <id|path>           # 按 ID 或外部文件路径恢复备份点
lkit backup delete <id>                 # 删除备份点
lkit config export                      # 导出 landscape_init.toml
lkit upgrade check [--json]             # 检查 Landscape 可升级版本
lkit upgrade apply [--version]          # 执行 Landscape 升级（默认 latest）
lkit rollback list [--json]             # 列出可回滚点
lkit rollback apply <id>                # 执行回滚（id 为备份点 ID）
lkit diagnose [--json]                  # 系统环境检查 + 诊断导出（V1 只读）
lkit self version                       # lkit 自身版本
lkit self upgrade check                 # 检查 lkit 新版本
# lkit self upgrade apply               # 升级 lkit 自身（V2）
lkit mirror sync [OPTIONS]              # 从上游同步 landscape release 到镜像目标
lkit mirror serve [OPTIONS]             # 启动镜像 HTTP 服务
lkit mirror verify [OPTIONS]            # 校验镜像完整性
lkit mirror list [OPTIONS]              # 列出已同步版本
```

> 镜像管理功能（`lkit mirror`）内置在 lkit 中，详见 [09-release-source.md](./09-release-source.md)。

**命令分类**：

| 类别 | 入口 | 说明 |
|---|---|---|
| 通用启动器 | `lkit`（无参数） | **唯一引导入口**，主菜单可导航至安装/备份/状态/升级/诊断等 |
| 非交互 | 其余所有命令 | 标准 CLI 模式，参数必填，直接执行 |

`lkit` 无参数时进入通用启动器，是所有交互操作的唯一入口。安装引导从启动器进入，而非 `install` 命令独立承担。其他子命令严格遵守 CLI 规范：参数必填、无交互提示、可通过 `--help` 查看用法。

> 详细网络初始化参数（WAN/LAN/PPPoE/DHCP）主要通过 `install` 引导流程收集；非交互场景使用 `--init-file`。

### 3.2 通用启动器与引导式安装

`lkit`（无参数）进入通用启动器，是唯一的交互入口。启动器主菜单覆盖：

- 安装与初始化
- 备份 / 恢复
- 升级 / 回滚
- 状态查看
- 日志查看
- 诊断

引导式安装作为启动器的一个入口，安装状态机（Precheck → Collect Config → Resolve Release → Fetch Artifacts → Apply Host Changes → First Boot → Finalize）定义见 [03-lifecycle](./03-lifecycle.md)。

1. **主机预检查**：系统依赖、运行环境
2. **安装源与版本**：多源并发探测（GitHub / HTTP 镜像 / 本地）、版本选择（详见 [09-release-source.md](./09-release-source.md)）
3. **安装路径与服务**：Landscape HOME、systemd 服务配置
4. **网络基础配置**：
   - 自动识别本机网卡
   - 选择 WAN / LAN 角色
   - WAN 接入方式：DHCP / Static IP / PPPoE
   - LAN IP / 子网
   - DHCP 开关与地址池
5. **管理入口配置**：API 监听等基础配置
6. **确认与执行**：摘要确认、执行安装、首次启动与健康检查

引导式安装的定位是 **安装/部署流程的一部分**，不是完整运行期网络配置器。

### 3.3 `lkit diagnose` 检查项

- 系统依赖检查（systemd、网络接口、必要命令）
- Landscape 进程状态与启动参数
- Landscape HOME 目录完整性
- 最近错误日志摘要
- 磁盘空间
- Landscape API 可达性

### 3.4 V1 明确不做

- 不做全量配置编辑器（V1 不替代 Web UI 的配置能力）
- 不做多机管理
- 不做插件系统
- 不做 Web UI
