# Software 面板样式验收标准

- 来源:侧栏面板;不依赖 Landscape 安装状态
- 对应实现:`console/software.rs`

## 布局

- 面板内单列段落:软件列表(当前为 Docker)与安装状态;Up/Down 移动焦点。

## 列表

- 软件行:选中 `FOCUS_SELECTED` 反色 + `> ` 标记;未选中默认色;
  进入面板默认选中唯一软件(Docker)并高亮;
- 安装状态:未安装灰字、已安装绿字/状态徽标;
- 发行版检测失败:红字错误;未安装软件按 Enter 打开确认层,
  已安装只显示提示。

## 确认层

- 居中带边框标题(含软件名);
- 首行加粗问题;来源切换行:**蓝底白字加粗** `◀ 官方 ▶`,
  可点击循环切换(官方仓库/阿里云/清华 TUNA/中科大 USTC);
- 切换提示行黄字;Enter 行白字、Esc 行灰字;
- 弹窗内点击=Enter、弹窗外=Esc;确认后不退出 alternate screen。

## 安装进度弹窗

- 居中带边框标题;阶段文案(准备软件源 / 安装软件包 / 启动服务)+
  青色 Gauge + 弹窗内醒目的 `Esc 取消安装` 提示行(黄字加粗);
- 安装期间按 Esc 打开取消确认层:Enter 确认取消(置位标志终止
  正在运行的软件包管理器命令,已写入源文件保留、下次覆盖),
  Esc 关闭继续安装;取消后面板恢复,可重新选择来源;
- 软件包子进程设置 PDEATHSIG,退出控制台(Ctrl+C)后自动终止不留残留;
- 弹窗内点击不触发动作,弹窗外=Esc。

## 证据

- `console/software.rs` `render_software`、`render_software_confirmation`、
  `render_software_progress`、`render_software_cancel_confirmation`;
- `software/docker.rs` `run_command`(取消轮询 + PDEATHSIG);
- 测试:`console/tests/software.rs`、`software/docker.rs` 的
  `run_command_cancels_the_running_child_process`。
