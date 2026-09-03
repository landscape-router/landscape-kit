# 阻塞接管屏样式验收标准

- 来源:安装根存在未完成网络接管(`awaiting_network_confirmation`、
  `finalizing`、`rolling_back`)时,TUI 启动即渲染阻塞屏而非菜单
- 对应实现:`console/network_wizard/render.rs` `render_pending_takeover`

## 布局

- 全屏无侧栏:标题 + 事务信息 + 两个选项行 + 灰字按键提示;
- 不渲染菜单、不启动环境检查或备份轮询,Install 菜单不可进入。

## 内容行

- 标题:黄字(网络接管等待确认,安装尚未提交);
- 事务信息:事务 ID、阶段、管理地址(DHCP 租约时显示占位)、
  确认截止时间、自动回滚提示;
- 自动回滚提示行过长时在弹窗内自动换行,不做截断;
- `rolling_back` 阶段显示回滚进行中说明,不提供"确认执行"。

## 选项行

- "稍后"(默认)与"确认执行":
  - 选中行 `FOCUS_SELECTED` 反色 + `> ` 标记;
  - 未选中默认色;
- "稍后":Enter/Esc/Ctrl+C 退出 TUI 回 shell;
- "确认执行":退出 TUI 后按命令行语义内联运行 `lkit network confirm`
  (普通终端输出,无全屏页,不限制 SSH 会话来源)。

## 证据

- `console/network_wizard/render.rs` `render_pending_takeover`;
- 测试:`console/tests/wizard.rs`、`console/tests/app.rs`
  (takeover pending 渲染与按键)。
