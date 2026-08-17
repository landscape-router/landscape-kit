# 全屏安装页样式验收标准

- 来源:Install 面板委托后的全屏操作页(无侧栏,退出 alternate screen 前)
- 对应实现:`interaction/presentation/screens/install.rs`

## 布局

- 三区:header(3 行:标题 + 底边框)/ body(进度区 + 日志面板)/ footer(3 行);
- 每个委托操作一个独立页面组件(`screens/` 下每操作一个文件),
  各自维护完整布局与文案,不复用其他操作的标题与结果框。

## Header

- 标题按状态变化,加粗 + 底边框:
  - 进行中:"正在安装 Landscape";
  - 成功:"安装完成";失败:"安装失败";取消:"安装已取消"。

## 进度区

- 下载型操作:青色 Gauge,标签含阶段名 + 百分比 + `已下载 / 总量`;
  下载阶段支持 Ctrl+C 停止,Esc 打开停止确认;
- 进入配置/网络/服务阶段后停止请求只显示提示并继续;
- 状态框(无进度事件时):成功时 `SUCCESS_BOX`(黑底绿字加粗),
  失败/取消默认色 + 对应文案。

## 日志面板(Output)

- 最近 8 行日志,带边框标题;
- 网络接管"等待确认"、"重新连接后运行 `lkit network confirm`"、
  "未在期限内确认将自动回滚"提示行用 `WARNING_HIGHLIGHT`
  (黑底黄字加粗)醒目标出,其余默认色;
- 日志面板可换行。

## Footer

- 灰字,超长自动换行;成功/失败/取消结果页显示
  "Ctrl+C 关闭",下载中显示停止/选项提示,其余阶段显示忽略提示。

## 停止确认层

- 居中 48x5,Enter 停止 / Esc 取消。

## 结果

- 结果页保持到 Ctrl+C;关闭结果页(或命令模式委托安装结束时)普通终端
  再输出一次结果提示(成功 `install: installation complete` /
  `安装完成`,失败含退出码)。

## 证据

- `interaction/presentation/screens/install.rs`;
- 测试:`renders_step_progress_gauge_for_stepped_operations`、
  `highlights_network_confirmation_lines_in_the_output_panel`
  (`screens/install.rs` 内嵌测试)。
