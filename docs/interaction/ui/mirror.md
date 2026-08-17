# Mirror 面板样式验收标准

- 来源:侧栏面板;不依赖 Landscape 安装状态
- 对应实现:`console/mirror.rs`

## 布局

- 面板内单列段落:主机摘要(发行版家族 + 软件包管理器)+ 镜像选项 +
  "恢复备份的原软件源"动作行;Up/Down 移动焦点。

## 列表与动作行

- 镜像选项:选中 `FOCUS_SELECTED` 反色 + `> ` 标记;未选中默认色;
- 探测状态:未知(探测失败)镜像灰字提示;
- "恢复备份的原软件源"动作行:聚焦高亮(反色标记),未聚焦默认色;
- 发行版检测失败:面板内红字错误。

## 确认层

- 居中带边框标题(`Confirm mirror switch` / `Confirm restore`);
- 首行加粗问题;计划行白字;
- 开关行:开启青色 `[x]`、关闭灰字 `[ ]`,可点击切换;Up/Down 在开关行间移动
  焦点(焦点行加粗),空格/←/→ 切换焦点行:
  - apt 家族:CD-ROM 源注释开关行(默认勾选);
  - Debian 家族:额外一行 security 仓库替换开关行(默认不勾选);
- 未知镜像:追加黄字警告行(换源可能失败);
- Enter 行白字、Esc 行灰字;
- 弹窗内点击=Enter、弹窗外=Esc;确认后在控制台内同步执行,结果写底栏,
  不退出 alternate screen。

## 证据

- `console/mirror.rs` `render_mirror`、`render_mirror_confirmation`;
- 测试:`console/tests/mirror.rs`。
