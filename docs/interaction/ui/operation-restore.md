# 全屏恢复页样式验收标准

- 来源:Backup 面板恢复确认后委托的全屏操作页
- 对应实现:`interaction/presentation/screens/restore.rs`

## 布局

与 [operation-install.md](operation-install.md) 相同的三区模板:
header(3 行)/ body(进度区 + 日志面板)/ footer(3 行)。

## 差异点

- 标题:进行中"正在恢复 Landscape";成功显示"恢复完成 / Restore complete"
  (独立文案,不复用安装页措辞),失败/取消各自独立标题;
- 恢复不发字节下载进度:按 systemd 4 步(准备 1/4 → 停止服务 2/4 →
  激活 3/4 → 初始化与健康检查 4/4)渲染百分比 Gauge,
  worker 在准备/停止服务/激活/初始化与健康检查阶段发送阶段与步骤事件;
- 日志面板与 Footer 样式同安装页;结果页状态框成功为 `SUCCESS_BOX`;
- 恢复确认已完成(`--console-confirmed`),不再请求 `/dev/tty` 二次确认。

## 证据

- `interaction/presentation/screens/restore.rs`;
- 测试:`renders_step_progress_gauge_for_stepped_operations`、
  `renders_restore_result_with_its_own_title_not_install_wording`、
  `restore_in_progress_hint_is_not_installation_wording`。
