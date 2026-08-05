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
