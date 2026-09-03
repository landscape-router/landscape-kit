# Update 面板样式验收标准

- 来源:侧栏面板;仅已安装时可用(否则置灰被导航跳过)
- 对应实现:`console/update.rs`

## 不可用状态(未安装)

- 黄字 "Landscape 未安装" + 灰字 "更新需要已安装 Landscape"。

## 字段与信息

- 顶部当前版本行:绿字(如 "当前版本  1.2.3");
- config.toml 损坏:红字错误行,不提供"当前来源"选项;
- 字段(与 Install 表单同款):
  - 当前项:`FOCUS_SELECTED` 反色 + `> ` 标记 + 整行青色背景,
    编辑中 `_` 光标;
  - 标签:未选中灰;值:白字;
  - "开始更新"动作项:未选中 `ACTION_HINT` 绿字 + 加粗;
  - 隐藏字段:自定义仓库 URL 仅自定义 HTTP 时显示;
- 解析中:底部青色"正在解析目标版本…",按键忽略。

## 解析分支(面板内提示,不退出控制台)

- 已是最新:面板内提示;降级:面板内错误;解析失败:面板内错误,可修改重试;
- 升级才打开居中确认层。

## 升级确认层

- 居中带边框标题 `Confirm update`;
- 首行加粗问题、`当前 X → 目标 Y` 计划行、复用 switch 流水线说明行、
  白字 Enter 行、灰字 Esc 取消行;
- Enter 确认、Esc 关闭;委托后的全屏页见
  [operation-update.md](operation-update.md)。

## 证据

- `console/update.rs` `render_update`、`render_update_confirmation`;
- 测试:`console/tests/update.rs`。
