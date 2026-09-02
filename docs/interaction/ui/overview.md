# Overview 面板样式验收标准

- 来源:侧栏面板(非全屏)
- 对应实现:`console/render.rs`(双栏布局、快照状态、daemon 动作行)

## 布局

宽面板(面板区域 >= 52 列)左右双栏,中间竖线分隔(左栏 `Borders::RIGHT`):

- 左栏:Landscape 安装信息
- 右栏:lkit 常驻服务(小节标题 + 版本 + daemon 状态 + 部署动作行)

窄面板(< 52 列,如 72 列终端)回退上下堆叠:先 Landscape 信息,空一行后
`SECTION_TITLE` 小节标题,再 lkit 常驻服务内容。

## 左栏:Landscape 安装信息

| 快照 | 首行文案 | 颜色 |
| --- | --- | --- |
| RootRequired | 需要 root 权限 | 黄 |
| NotInstalled | Landscape 未安装 | 黄 |
| Installed | Landscape 已安装 | 绿 |
| Unavailable | 安装状态需要关注 | 红 |
| AwaitingNetworkConfirmation | 网络接管待确认标题 | 黄 |

首行之后为白字详情(版本 / 服务 manager / 初始化状态 / 安装根),不同快照
展示对应字段;Unavailable 附带错误文本。

## 右栏:lkit 常驻服务

- 小节标题:`SECTION_TITLE`(灰 + 加粗);
- 服务简介:标题下一行灰色简介(`overview_lkit_section_help`,说明常驻服务以
  systemd 常驻并代控制台执行特权操作),按栏宽预折行(词级断行,超宽无空格段
  按字符硬切,见 `widgets::wrap_to_width`),保证动作行命中区行号与渲染一致;
- lkit 版本行:白字,值为当前二进制版本(`env!("CARGO_PKG_VERSION")`),
  daemon 未运行时也显示;
- daemon 状态行:运行中绿字、未运行红字(文案同 `console.overview_lkit_*`);
- 部署动作行(仅 daemon 未运行时):
  - 聚焦时:`FOCUS_SELECTED` 反色(黑底青字 + 加粗)+ `> ` 标记;
  - 未聚焦时:`ACTION_HINT` 绿字 + 加粗,`> ` 为等宽空格;
  - 点击行等价于 Enter(命中区注册在右栏坐标);
- 恢复码简介 + 查看动作行(仅 daemon 运行时):动作行上方一行灰色简介
  (`overview_lkit_psk_help`,说明急救恢复码用于常规网络失联时的 L2 flare
  急救通道),动作行样式与点击语义同上。

## Header 徽标

- `Landscape Kit` 白字加粗 + 两个状态徽标(见 [总样式](README.md#header));
- Landscape 徽标带主语(如 `Landscape: installed` / `Landscape: not installed`),
  语义 = 左栏快照;daemon 徽标 = `daemon_is_running()`;
- 窄终端放不下时徽标全部隐藏。

## 证据

- 双栏布局与快照行:`console/render.rs` `render_overview`、`overview_landscape_lines`、
  `overview_lkit_lines`;
- 部署动作行点击命中:`console/tests/daemon.rs` `overview_shows_daemon_status_*`、
  `mouse_click_on_deploy_row_opens_the_confirm_layer`。
