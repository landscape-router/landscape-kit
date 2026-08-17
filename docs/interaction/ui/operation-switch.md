# 全屏切换页样式验收标准

- 来源:Update 面板(或 switch 命令)委托后的全屏操作页
- 对应实现:`interaction/presentation/screens/switch.rs`

## 布局

与 [operation-install.md](operation-install.md) 相同的三区模板:
header(3 行)/ body(进度区 + 日志面板)/ footer(3 行)。

## 差异点

- 标题:进行中"正在切换 Landscape";成功"切换完成"、
  失败/取消各自独立文案,不复用安装页措辞;
- 切换使用步骤进度条(准备/停止服务/激活/验证阶段事件,如 `2/4`),
  青色 Gauge 显示百分比与阶段文本;无字节下载进度;
- 日志面板与 Footer 样式同安装页;无下载阶段,不做停止确认层;
- 结果页状态框:成功 `SUCCESS_BOX`(黑底绿字加粗)。

## 证据

- `interaction/presentation/screens/switch.rs`;
- 测试:`renders_step_progress_gauge_for_stepped_operations`
  (`screens/switch.rs` 内嵌测试)。
