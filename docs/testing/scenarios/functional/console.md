# Ratatui 管理控制台场景

## UI-01

**裸 lkit 显示固定侧栏与安装面板**

- 测试层：Clap 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：内存终端断言品牌、Navigation、Install root 和安装操作均已渲染，不要求测试进程
  连接真实终端；同时断言 Repository URL 默认隐藏、仅在选择 Custom HTTP 后显示，字段导航
  会跳过隐藏行，并验证所有安装字段都有随选择变化的说明。

## UI-02

**安装表单生成与 CLI 等价的结构化请求**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
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
- 证据：[控制台输入规格](../../../interaction/console.md)、[控制台按键测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：覆盖从侧栏进入 Install 面板后使用 Left 返回侧栏，并断言 Left 不会修改当前枚举；
  Right 仍可切换安装选项。

## UI-05

**Install 后台运行并展开部署前检查结果**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)、[`lkit check` 规格](../../../check.md)
- 说明：覆盖检查汇总、检查与表单间的焦点移动、分组详情、非通过原因与建议，以及 Esc 收起
  详情而不触发退出确认；详情底栏同时显示 `Ctrl+C Exit` 和 `Esc Close`，明确区分退出控制台
  与收起详情；检查任务通过后台线程运行。

## UI-06

**控制台即时切换并在底栏显示语言**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台输入规格](../../../interaction/console.md)、[本地化规格](../../../interaction/i18n.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：断言英文底栏、`L` 切换后的中文导航与中文底栏，并验证文本编辑状态下 `l` 仍写入字段而不切换语言。

## UI-07

**右侧面板焦点在基础终端颜色下保持可见**

- 测试层：Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台输入规格](../../../interaction/console.md)、[控制台渲染测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：Overview 和 Install 面板标题显示 `> ` 焦点标记；Install 当前字段使用 `> ` 和基础
  Cyan 背景，不依赖 truecolor 支持。

## UI-08

**网络接管从 Install 表单进入无侧栏全屏向导**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：覆盖从 Install 表单进入无侧栏网络向导、LAN 空集合的 WAN-only 计划，以及 LAN
  列表的 Up/Down、Space、Enter 语义。systemd worker 的安装页在下载阶段可停止，配置阶段
  忽略停止请求，结果页等待 Ctrl+C。

## UI-09

**环境检查门禁阻止不安全的 Install 操作**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：NotRun/Running 时焦点停留在检查汇总并提示等待；Pass 和 warning 可进入表单；
  Error、unknown 和 worker 失败通过处理弹窗阻断所有进入表单、开始安装和网络向导的路径。
  弹窗支持 Enter 查看详情、Esc 关闭、R 重跑，无强制跳过入口；进入表单后重跑变为阻断状态时，
  “开始安装”与网络向导入口激活前同样复查。

## UI-10

**网络向导预填与计划摘要确认**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
- 说明：WAN 列表显示首个发现 IPv4 和该接口首个默认网关；选中后以完整对预填并默认
  Static，缺任一项默认 DHCP。Static/DHCP 使用 Left/Right 和 Enter，静态地址/CIDR 与网关
  同页编辑。计划摘要展示 WAN、LAN、LAN 配置和接管影响；Enter 开始安装，Esc 逐步回退，
  在 WAN 首页打开取消确认层。

## UI-11

**Backup 面板列出备份并支持创建、详情与恢复**

- 测试层：Rust 单元、Ratatui TestBackend
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[备份命令](../../../commands/backup.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)
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
- 证据：[控制台规格](../../../interaction/console.md)、[`lkit update`](../../../commands/update.md)、[控制台测试](../../../../crates/lkit-cli/src/console.rs)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：已安装时 Update 菜单可选，未安装/非 root/状态不可读时置灰且导航跳过。面板顶部
  显示当前版本，字段为目标版本（默认 latest，`TargetVersion` 校验）、仓库来源（config.toml
  有效时首项为“当前来源”，损坏时显示错误且只留显式选项）与自定义 URL。激活“开始更新”后
  在后台线程解析目标版本：解析中忽略按键；已是最新与降级在面板内提示且不退出；解析失败
  显示错误；升级才显示确认层（`当前 <X> → 目标 <Y>`，Y 为解析出的真实版本），Enter 构建
  带 `--console-confirmed` 的结构化 `Update` 请求与显式 `--repository` 参数，Esc 取消。
- 缺口：真实网络解析与 worker 全屏更新页的执行链路依赖安装现场，控制台单测覆盖状态机、
  渲染与请求构建；`--console-confirmed` 跳过 tty 由命令层测试覆盖。
