# Reinit 面板样式验收标准

- 来源:侧栏面板;仅已安装、systemd 且宿主网络已被接管时可用,否则置灰跳过
- 对应实现:`console/reinit.rs`

## 布局

- 面板内单列段落:版本与服务摘要、reinit 说明、"开始 reinit"动作行、
  凭据字段、新计划摘要、"重新初始化 Landscape"动作行。

## 摘要与说明

- 版本行:绿字(版本 + manager + 初始化状态);
- reinit 说明(清空范围、重建数据库)与凭据步骤说明:灰字;
- 未接管警告/不可用原因:红字或黄字,面板显示原因且菜单被跳过。

## 动作行与凭据字段

- "开始 reinit"动作行:聚焦时 `> ` 光标标记 + 高亮(`FOCUS_SELECTED` 反色),
  Enter 进入与 Install 相同的全屏网络向导(见
  [network-wizard.md](network-wizard.md));
- 凭据字段(向导返回后):管理员用户名、密码、密码确认,
  密码以等长 `*` 掩码;当前字段 `FOCUS_SELECTED` 反色 + `> ` 标记,
  与 Install 表单同款(标签灰、值白、编辑中 `_` 光标);
- "重新初始化 Landscape"动作行:未聚焦 `ACTION_HINT` 绿字 + 加粗,
  聚焦 `FOCUS_SELECTED`;
- 新计划摘要行:白字(WAN 与 LAN,未选 LAN 显示"无")。

## 确认层

- 居中带边框标题 `Confirm reinit`;
- 首行加粗问题、清空范围行、保护 `.lkb` 备份行、确认窗口
  (等待 `lkit network confirm`)说明行、白字 Enter 行、灰字 Esc 行;
- 弹窗内点击=Enter、弹窗外=Esc;委托后的全屏页见
  [operation-reinit.md](operation-reinit.md)。

## 证据

- `console/reinit.rs` `render_reinit`、`render_reinit_confirmation`;
- 测试:`console/tests/reinit.rs`。
