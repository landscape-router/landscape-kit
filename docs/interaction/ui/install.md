# Install 面板样式验收标准

- 来源:侧栏面板;未安装时可用,已安装时置灰并被导航跳过
- 对应实现:`console/install_form.rs`、`console/preflight.rs`

## 布局

- 左表单 + 右帮助区(面板内容区 >= 72 列);窄屏帮助区移到表单下方
  (高度允许时);更窄只渲染表单;
- 面板标题:panel_block 焦点边框(聚焦时 `> Install` + 青色加粗边框)。

## 检查汇总(表单第一个焦点项)

- 选中时:`FOCUS_SELECTED` 反色 + `> ` 标记 + 整行青色背景;
- 状态标签颜色(`check_status_color`):
  - 未运行 NotRun:灰;运行中 Running:青;worker 失败 Failed:红;
  - Pass 绿 / Warning 黄 / Error 红 / Unknown 品红;
- 展开详情:组标题加粗、检查标题白字、reason 灰、suggestion 黄;
  详情底栏同时显示 `Ctrl+C Exit` 与 `Esc Close`;
- 阻塞时显示居中处理弹窗(见下)。

## 表单字段

- 当前项:`FOCUS_SELECTED` 反色 + 固定宽度 `> ` 标记 + 整行青色背景,
  编辑中行尾 `_` 光标;
- 标签:未选中灰;值:显式白字;
- 密码/确认密码:等长 `*` 掩码;
- 隐藏字段(Repository URL):仓库类型为 Custom HTTP 时才显示,导航跳过;
- "开始安装"动作项:未选中 `ACTION_HINT` 绿字 + 加粗,选中 `FOCUS_SELECTED`;
- 帮助区:灰字显示当前项配置含义与影响。

## 不可用状态(已安装)

- 首行绿字 "Landscape 已安装" + 灰字原因说明(panel_block 正常边框)。

## 阻断弹窗(环境检查)

- 居中带边框标题 `Install blocked`;
- 阻断项列表 + 建议;
- daemon 未运行被阻断时含"部署 lkit 常驻服务"按钮:
  **常显 `FOCUS_SELECTED` 反色**(弹窗无焦点环,它是唯一要突出的动作,
  鼠标点击与 D 键都直接部署);
- 底行灰字操作提示(Enter 查看详情 / Esc 关闭 / R 重跑,daemon 阻断时含 D 部署);
- 弹窗内点击=Enter、弹窗外=Esc。

## 证据

- 表单与掩码:`console/install_form.rs` `render_install_form`;
- 检查汇总与弹窗:`console/preflight.rs` `render_preflight_summary`、
  `render_preflight_dialog`;
- 测试:`console/tests/install.rs`、`console/tests/daemon.rs`
  `preflight_dialog_*`。
