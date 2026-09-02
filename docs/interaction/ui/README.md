# lkit TUI 样式验收标准(总样式)

本文集中定义 Ratatui 管理控制台的全局布局、样式令牌与公共组件。各页面自身的样式
验收标准见本目录下对应文件;行为与交互语义见 [Ratatui 管理控制台](../console.md)。

## 布局骨架

正常尺寸下控制台为三区结构(`console/render.rs` `render`):

- header:2 行(品牌标题 + 状态徽标,紧贴底部边框,无空行)
- body:左侧栏(24 列)+ 右侧功能面板(`Min(24)` 起)
- status:动态高度——1 行顶边框 + 状态行(至少 1 行,超长换行)+ 提示行
  (至少 1 行,超长换行);短内容共 3 行,长内容最多 5 行,不留空行

终端小于 72x18 时不渲染骨架,居中显示"终端尺寸过小"提示与 `Landscape Kit`
边框标题。全屏页(网络向导、阻塞接管屏、委托操作页)无侧栏,自行定义
header/body/footer。

## 样式令牌

令牌只描述视觉特征,不绑定具体颜色值;实现分布在 `console/render.rs`、
`console/backup/render.rs`、`console/install_form.rs`、`interaction/presentation/*`。

| 令牌 | 视觉特征 | 用途 |
| --- | --- | --- |
| `FOCUS_SELECTED` | 黑底青字 + 加粗 | 选中项反色:侧栏高亮、表单当前项、列表选中行、向导选中行、动作行聚焦态 |
| `ACTION_HINT` | 绿字 + 加粗 | 可执行动作行未聚焦态(开始安装/开始更新/创建备份/部署 daemon) |
| `WARNING_HIGHLIGHT` | 黑底黄字 + 加粗 | 操作页 Output 面板中网络接管"等待确认/确认/自动回滚"提示行 |
| `SUCCESS_BOX` | 黑底绿字 + 加粗 | 委托操作页结果状态框(成功) |
| `MUTED` | 灰色 | 次要信息、不可用菜单、操作提示、底栏 hints |
| `ACCENT` | 青色 | 进行中、焦点边框、进度 Gauge、语言指示 |
| `SECTION_TITLE` | 灰色 + 加粗 | 面板内小节标题(Overview 左右栏、堆叠模式小节) |
| `INVALID_ENTRY` | 红字 | 损坏条目(备份 invalid、错误状态) |
| 状态色 | 绿=正常/运行中、红=错误/未运行、黄=警告/待注意 | 状态行与徽标 |
| 检查状态色 | Pass 绿 / Warning 黄 / Error 红 / Unknown 品红 | 检查汇总与详情 |
| 详情颜色 | reason 灰、suggestion 黄 | 检查详情行的原因与建议 |

## 焦点标记与焦点边框

- 焦点标记:固定宽度 `> ` 前缀(选中行),未选中为等宽空格 `  `,保证不抖动。
  用于侧栏选中项、表单当前项、动作行、向导列表选中行、面板标题。
- 焦点边框:面板聚焦时标题带 `> ` 前缀且边框为青色加粗,未聚焦普通边框
  (`console/render.rs` `panel_block`)。

## Header

- `Landscape Kit` 白字加粗,右侧两个状态徽标:
  - Landscape 安装状态(带主语):`Landscape: installed` 绿 /
    `Landscape: not installed` 黄 / `Landscape: root required` 黄 /
    `Landscape: attention required` 红(来自安装快照);
  - lkit daemon 状态:`daemon: running` 绿 / `daemon: not running` 红
    (`daemon_is_running()`);
  - 窄终端放不下时两个徽标全部隐藏,只保留品牌标题。

## Status 区(底栏)

- 动态高度(1 边框 + 状态行 + 提示行,均至少 1 行、超长换行);
- 状态行:当前 notice,按级别染色——`Ready` 灰、过程信息/引导提示黄、操作成功
  绿、失败/阻断红(类型为 `console::notice::Notice`,取代旧的"非 Ready 一律
  红字"约定),超长换行;右下角语言指示
  青色右对齐,可切换时显示**目标语言**与 `[L]` 按键提示(`[L] Switch to 中文 (zh)` /
  `[L] 切换到 English (en)`),按 `L` 或点击切换到所示目标;不可切换(编辑中)时
  只显示当前语言(`Language: English (en)` / `语言：中文 (zh)`);
- 提示行:按焦点显示可用操作,`Ctrl+C Exit` 置首,超长换行不截断。

## 公共弹层

- 确认层:居中,先 `Clear` 再带边框标题渲染;弹窗内点击=Enter、弹窗外=Esc;
  首行问题加粗,`Esc` 取消行灰色;
- 进度弹层:居中,内部不响应点击(输入框除外),弹窗外=Esc;阶段文案 + Gauge
  (青色)+ 灰色提示行;
- 退出确认层 48x7、停止确认层 48x5,均居中;
- 弹层命中区后注册者优先,弹层覆盖底层界面。

## 页面清单

| 页面 | 文件 | 对应实现 |
| --- | --- | --- |
| Overview | [overview.md](overview.md) | `console/render.rs` |
| Install | [install.md](install.md) | `console/install_form.rs`、`console/preflight.rs` |
| Backup | [backup.md](backup.md) | `console/backup/render.rs` |
| Update | [update.md](update.md) | `console/update.rs` |
| Mirror | [mirror.md](mirror.md) | `console/mirror.rs` |
| Software | [software.md](software.md) | `console/software.rs` |
| Reinit | [reinit.md](reinit.md) | `console/reinit.rs` |
| 网络向导 | [network-wizard.md](network-wizard.md) | `console/network_wizard/render.rs` |
| 阻塞接管屏 | [pending-takeover.md](pending-takeover.md) | `console/network_wizard/render.rs` |
| 全屏安装页 | [operation-install.md](operation-install.md) | `interaction/presentation/screens/install.rs` |
| 全屏切换页 | [operation-switch.md](operation-switch.md) | `interaction/presentation/screens/switch.rs` |
| 全屏更新页 | [operation-update.md](operation-update.md) | `interaction/presentation/screens/update.rs` |
| 全屏修复页 | [operation-repair.md](operation-repair.md) | `interaction/presentation/screens/repair.rs` |
| 全屏恢复页 | [operation-restore.md](operation-restore.md) | `interaction/presentation/screens/restore.rs` |
| 全屏重初始化页 | [operation-reinit.md](operation-reinit.md) | `interaction/presentation/screens/reinit.rs` |
