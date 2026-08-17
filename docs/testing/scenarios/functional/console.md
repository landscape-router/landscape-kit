# Ratatui 管理控制台场景

## UI-01

**裸 lkit 显示固定侧栏与安装面板**

- 测试层：Clap 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：内存终端断言品牌、Navigation、Install root 和安装操作均已渲染，不要求测试进程
  连接真实终端；同时断言 Repository URL 默认隐藏、仅在选择 Custom HTTP 后显示，字段导航
  会跳过隐藏行，并验证所有安装字段都有随选择变化的说明。网络接管开关暂隐藏（固定启用，
  见代码中的 `TODO(network-takeover)`），不在表单字段中。

## UI-02

**安装表单生成与 CLI 等价的结构化请求**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：覆盖精确版本、自定义 HTTP 仓库、安装根目录、管理员、双重掩码密码和 service
  manager；断言密码不出现在 Debug 或 CLI args，并在离开控制台后进入共享命令分发与
  systemd worker 判断。

## UI-03

**控制台退出恢复 raw mode、alternate screen 与光标**

- 测试层：PTY CLI fixture E2E
- 状态：`已覆盖`
- 证据：[控制台恢复契约](../../../interaction/console.md)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：真实 PTY 驱动裸 `lkit`，断言第一次 Esc 只进入等待状态、第二次 Esc 才显示确认层，
  Enter 确认后离开 alternate screen；Ctrl+C 仍立即返回 130，且两条路径均保持 ECHO 启用。

## UI-04

**Install 面板可使用左方向键返回侧栏**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[控制台输入规格](../../../interaction/console.md)、[控制台按键测试](../../../../crates/lkit-cli/src/console/)
- 说明：覆盖从侧栏进入 Install 面板后使用 Left 返回侧栏，并断言 Left 不会修改当前枚举；
  Right 仍可切换安装选项。

## UI-05

**Install 后台运行并展开部署前检查结果**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)、[`lkit check` 规格](../../../check.md)
- 说明：覆盖检查汇总、检查与表单间的焦点移动、分组详情、非通过原因与建议，以及 Esc 收起
  详情而不触发退出确认；详情底栏同时显示 `Ctrl+C Exit` 和 `Esc Close`，明确区分退出控制台
  与收起详情；检查任务通过后台线程运行。

## UI-06

**控制台即时切换并在底栏显示语言**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台输入规格](../../../interaction/console.md)、[本地化规格](../../../interaction/i18n.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：断言英文底栏、`L` 切换后的中文导航与中文底栏，并验证文本编辑状态下 `l` 仍写入字段而不切换语言。

## UI-07

**右侧面板焦点在基础终端颜色下保持可见**

- 测试层：Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台输入规格](../../../interaction/console.md)、[控制台渲染测试](../../../../crates/lkit-cli/src/console/)
- 说明：Overview 和 Install 面板标题显示 `> ` 焦点标记；Install 当前字段使用 `> ` 和基础
  Cyan 背景，不依赖 truecolor 支持。

## UI-08

**网络接管从 Install 表单进入无侧栏全屏向导**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：覆盖从 Install 表单进入无侧栏网络向导、WAN 配置面板（Static/DHCP client tab、
  静态字段与底部“确认并继续”按钮）、LAN 空集合的 WAN-only 计划，以及 LAN
  列表的 Up/Down、Space、Enter 语义。Install 面板始终启用网络接管（开关暂隐藏），激活
  “开始安装”固定进入向导。systemd worker 的安装页在下载阶段可停止，配置阶段
  忽略停止请求，结果页等待 Ctrl+C。

## UI-09

**环境检查门禁阻止不安全的 Install 操作**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：NotRun/Running 时焦点停留在检查汇总并提示等待；Pass 和 warning 可进入表单；
  Error、unknown 和 worker 失败通过处理弹窗阻断所有进入表单、开始安装和网络向导的路径。
  弹窗支持 Enter 查看详情、Esc 关闭、R 重跑，无强制跳过入口；进入表单后重跑变为阻断状态时，
  “开始安装”与网络向导入口激活前同样复查。

## UI-10

**网络向导预填与计划摘要确认**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：WAN 列表显示首个发现 IPv4 和该接口首个默认网关；选中后进入 WAN 配置面板，
  tab 以完整对预填并默认 Static，缺任一项默认 DHCP。Static/DHCP 用 Left/Right 切换，
  静态地址/CIDR 与网关同页编辑，底部“确认并继续”按钮校验并前进；选择 LAN 后在同一页
  填写管理地址与 DHCP 范围并一次性确认。计划摘要展示 WAN、LAN、LAN 配置和接管影响；
  Enter 开始安装，Esc 逐步回退，在 WAN 首页打开取消确认层。

## UI-11

**Backup 面板列出备份并支持创建、详情与恢复**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[备份命令](../../../commands/backup.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：未安装或非 root 时面板只显示原因提示。已安装时列出备份（与 `backup list`
  同源的后台完整校验），Enter 打开 metadata 详情（V 后台完整 verify，R 恢复），顶部
  创建动作支持备注编辑并在 Enter 后生成与 CLI 等价的结构化 `Backup`/`Restore` 请求；
  恢复确认层 Enter 提交、Esc 取消，`--yes` 由控制台确认层覆盖。
- 缺口：真实备份文件的列表加载与损坏条目标记依赖安装现场，测试通过注入 metadata 覆盖
  渲染与按键路径；后台 verify 与恢复委托 worker 的端到端执行未自动化。

## UI-12

**Update 面板：字段、后台解析分支、确认层与结构化请求**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[`lkit update`](../../../commands/update.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：已安装时 Update 菜单可选，未安装/非 root/状态不可读时置灰且导航跳过。面板顶部
  显示当前版本，字段为目标版本（默认 latest，`TargetVersion` 校验）、仓库来源（config.toml
  有效时首项为“当前来源”，损坏时显示错误且只留显式选项）与自定义 URL。激活“开始更新”后
  在后台线程解析目标版本：解析中忽略按键；已是最新与降级在面板内提示且不退出；解析失败
  显示错误；升级才显示确认层（`当前 <X> → 目标 <Y>`，Y 为解析出的真实版本），Enter 构建
  带 `--console-confirmed` 的结构化 `Update` 请求与显式 `--repository` 参数，Esc 取消。
- 缺口：真实网络解析与 worker 全屏更新页的执行链路依赖安装现场，控制台单测覆盖状态机、
  渲染与请求构建；`--console-confirmed` 跳过 tty 由命令层测试覆盖。

## UI-13

**待确认网络接管进入 TUI 时显示阻塞屏，不进入菜单**

- 测试层：Rust 单元、Ratatui TestBackend、PTY CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：安装根存在未完成网络接管（`awaiting_network_confirmation`、`finalizing`、
  `rolling_back`）时，快照进入 `AwaitingNetworkConfirmation`，TUI 启动即渲染阻塞屏而非
  菜单：显示事务 ID、阶段、管理地址（DHCP 租约时显示占位）、确认截止时间与回滚提示。
  “稍后”（默认，Enter/Esc/Ctrl+C）退出 TUI 回 shell；↑/↓ 或 Tab 选择“确认执行”后 Enter
  返回与 CLI 等价的结构化 `Network confirm` 请求，退出 TUI 后按现状命令行语义内联执行
  （不限制 SSH 会话来源）。`rolling_back` 阶段“确认执行”不可用，只留“稍后”。
- 缺口：PTY E2E 在非 root 环境跳过（快照显示 RootRequired），阻塞屏的完整端到端路径
  只在 root 环境验证；确认执行的委托 worker 全屏路径与 QEMU 现场未自动化。

## UI-14

**daemon 运行状态在 check、进入控制台与 Overview 面板提前展示并可部署**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`部分覆盖`
- 证据：[`lkit check` 规格](../../../check.md)、[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)、[daemon_worker 测试](../../../../crates/lkit-cli/src/daemon_worker/mod.rs)
- 说明：`lkit check` 与 Install 面板部署前检查包含 `service.lkit_daemon` 项：daemon
  运行中为 `pass`；root 下未运行为 `error` 并建议 `lkit self install`（控制台未部署
  daemon 前无法进入安装表单）；非 root 未运行只报 `warning`。进入控制台时 root 下
  daemon 未运行，底栏提示行直接显示警告；Overview 面板常驻显示 daemon 运行状态行，
  未运行时显示“部署 lkit 常驻服务”动作行：Enter 打开确认层，确认后在 TUI 内后台
  线程执行 `lkit self install`（进度弹层，结果写底栏，不退出控制台），成功后状态行
  变绿、动作行消失、预检自动重跑。安装阻断弹框内因 daemon 检查被拦时直接提供部署
  按钮（`D` 键或点击），部署完成后表单门禁自动放行。
- 缺口：`delegation_blocked` 的 root 分支依赖真实 euid，标准单测环境（非 root）只
  覆盖 `daemon_is_running` 的 pidfile 语义、检查函数分支与部署后台线程的失败路径
  （非 root 得到 root 权限错误）；root 环境的真实 systemd 部署与 TUI 现场（含部署
  成功后预检自动重跑）待补充。

## UI-15

**激活“开始安装”与网络向导确认摘要前复查 daemon，未运行不退出控制台**

- 测试层：Rust 单元
- 状态：`部分覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console/)
- 说明：root 下 daemon 未运行（含检查结果过时的情况）时，“开始安装”与网络向导
  确认摘要不再退出 TUI 委托，而是留在面板内提示“lkit 常驻服务未运行;请用
  `lkit self install` 部署”，避免用户填写完所有安装参数、退出控制台后才得到
  `the lkit daemon is not running` 错误（CLI 命令模式仍由 `delegate()` 以退出码 `2`
  拒绝，见 [`SYS-03`](../systemd-smoke.md#sys-03)）。
- 缺口：root 分支依赖真实 euid 与地盘 pidfile，标准单测只覆盖非 root（不阻断）
  路径；root 环境的 TUI 现场行为待补充。
