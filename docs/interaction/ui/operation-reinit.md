# 全屏重初始化页样式验收标准

- 来源:Reinit 面板确认后委托的全屏操作页
- 对应实现:`interaction/presentation/screens/reinit.rs`

## 布局

与 [operation-install.md](operation-install.md) 相同的三区模板:
header(3 行)/ body(进度区 + 日志面板)/ footer(3 行)。

## 差异点

- 标题:进行中"正在重新初始化 Landscape";成功/失败/取消各自独立文案;
- reinit 无字节下载,按准备、停止服务、激活与健康检查阶段渲染
  百分比 Gauge(步骤进度条);
- 日志面板与 Footer 样式同安装页;结果页状态框成功为 `SUCCESS_BOX`;
- 成功进入待确认状态后由 `lkit network confirm` / `rollback` 收尾。

## 证据

- `interaction/presentation/screens/reinit.rs`。
