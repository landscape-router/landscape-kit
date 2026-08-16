# 输出与退出码

管理操作输出顺序：

1. 安装根目录和当前状态；
2. 未完成事务处理；
3. 部署前检查；
4. 仓库和目标版本；
5. 下载与校验；
6. `.lkb` 备份；
7. 激活与健康检查；
8. 成功结果或回滚结果；
9. 后续人工提醒。

`backup create` 输出备份 ID、文件路径、Landscape 版本、scope 和 warning；`backup list`、
`show`、`verify` 输出稳定的字段顺序，不输出归档中的初始化配置内容。`restore` 在破坏性
确认前先输出当前版本、目标版本、备份 ID 和“minimal 不含数据库”warning，之后沿用事务阶段、
健康检查和回滚结果的顺序。`uninstall` 在破坏性确认前先输出当前版本、服务状态、数据损失
范围与保留物清单，检测到网络接管特征时追加宿主网络服务警告，之后沿用事务阶段顺序并
在成功时输出保护备份 ID 与保留物。`self` 命令输出被操作对象与结果（安装/升级/移除的
daemon 或 `/usr/local/bin/lkit`），`upgrade` 输出目标版本与校验结果。

交互终端中的 warning 使用标题、原因和建议分行显示。资产下载使用动态进度条；非终端
stderr 不输出进度控制字符。systemd worker 只写结构化进度事件，由仍然连接且 stderr 为
终端的前端使用 Ratatui inline viewport 渲染；普通 stdout/stderr 仍按事务规范记录并转发。
CI、`Command::output()` 和其他非交互调用只消费进度事件，不产生 ANSI 控制序列。
显式 `--non-interactive` 在终端存在时也禁用提示和 Ratatui inline 渲染。

用户可见文案支持英文和简体中文，语言选择与不翻译的机器契约见
[命令行本地化](../interaction/i18n.md)。语言只改变文案，不改变输出顺序、结构化事件、
状态键或退出码。

不得输出：

- 管理员密码；
- 初始化配置内容；
- API token；
- 证书私钥；
- 带 query 或 fragment 的预签名 URL。

退出码：

- `0`：目标状态达到；只有 warning 且安装成功时仍返回 `0`；
- `1`：普通失败，当前已提交安装状态未被破坏，包括用户拒绝、安装锁冲突、首次安装失败和 `--force` 提示手工清理；
- `2`：CLI 参数或参数组合错误，沿用 Clap 的用法错误退出码；
- `5`：目标版本、后端 repair、restore 或迁移激活失败，但原安装状态（迁移场景为旧
  实例）自动恢复成功；
- `6`：回滚失败、事务或状态损坏，或者需要人工恢复。
- `130`：进程收到显式 Ctrl+C；该状态是信号取消结果，不是业务失败码。

`uninstall` 没有自动回滚语义，只使用 `0/1/2` 和 `130`，不定义 `5` 和 `6`；卸载中断
恢复采用前向完成，失败时保护 `.lkb` 与事务现场保留在 lkit 地盘，见
[`lkit uninstall`](../commands/uninstall.md#失败语义)。`self` 系列不创建事务、不定义
`5` 和 `6`；`upgrade` 与目标版本相同或已是最新时返回 `0`，版本参数非法返回 `2`。

除业务码 `0/1/2/5/6` 和信号取消状态 `130` 外，v1 不定义其他 `lkit` 管理命令退出码。
生产 systemd 环境中的前端保持连接时，返回 worker 记录的业务命令退出码；显式 Ctrl+C
停止临时 worker 后返回 `130`。SSH 或终端断开时，调用方可能收不到退出状态，但 systemd
worker 继续完成提交或自动回滚。worker 或主机重启导致事务中断时，不伪造本次退出码；
下一次执行先按中断恢复规则处理。

## 首版非目标

- 不自动卸载 NetworkManager。
- 不自动停止、禁用或 mask `systemd-resolved`。
- 不自动修改 `/etc/network/interfaces` 或选择 WAN/LAN 网卡。
- 不自动修改防火墙、SELinux、sysctl、Cgroup 或内核配置。
- 不自动安装系统软件包、PPP 或容器运行时。
- `install` 不迁移 `/root/.landscape-router` 等旧手工部署；迁移使用 `lkit migrate`
  （备份 → 确认 → 停止旧实例 → 重建 → 接管），见 [`lkit migrate`](migrate.md)。
- 当前发布产物不支持 Alpine 等 musl 发行版。
- 不支持 `x86_64` 和 `aarch64` 以外架构。
- 不允许安装 prerelease。
- 不实现 AWS S3 profile、region 或 access key 鉴权。
- 不自动删除旧版本、事务或 `.lkb`；`uninstall` 是唯一显式清理入口，删除 landscape 根
  受管内容并保留 lkit 地盘（`config.toml`、`backups/` 与 `transactions/`），不支持
  `--purge-root`。
- 网络接管未确认回滚成功时删除本次未提交的整个 `data/`；已提交安装、旧版本和事务日志
  不在该清理范围内。
- 不实现 `.lkb` full scope。
- 不备份数据库、API token、日志和指标历史。
- 不支持自定义端口。
- 除网络接管未确认回滚按事务契约清理未提交首次安装的 `data/`、以及 `uninstall` 按
  命令规格清理已提交安装外，不自动清理安装目录；v1 的 `--force` 只提示用户手工处理。
