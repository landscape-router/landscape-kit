# 全屏更新页样式验收标准

- 来源:Update 面板确认后委托的全屏操作页
- 对应实现:`interaction/presentation/screens/update.rs`

## 布局

与 [operation-install.md](operation-install.md) 相同的三区模板:
header(3 行)/ body(进度区 + 日志面板)/ footer(3 行)。

## 差异点

- 标题:进行中"正在更新 Landscape";成功/失败/取消各自独立文案;
- 更新复用 switch 流水线:显示下载、配置与服务阶段;
  下载阶段进度条青色;步骤型阶段用百分比 Gauge;
- 日志面板与 Footer 样式同安装页;结果页状态框成功为 `SUCCESS_BOX`。

## 证据

- `interaction/presentation/screens/update.rs`。
