# Software 面板样式验收标准

- 来源:侧栏面板;不依赖 Landscape 安装状态
- 对应实现:`console/software.rs`

## 布局

- 面板内单列段落:软件列表(Docker)与安装状态,空行后是基础包行;
  Up/Down 移动焦点(两行间循环,两端钳制)。

## 列表

- 软件行:选中 `FOCUS_SELECTED` 反色 + `> ` 标记;未选中默认色;
  进入面板默认选中唯一软件(Docker)并高亮;
- 安装状态:未安装灰字、已安装绿字/状态徽标;
- 基础包行:显示缺失数量(黄字 `缺 n 个`)或已安装(绿字);
  Enter 打开基础包多选弹框;
- 发行版检测失败:红字错误;未安装软件按 Enter 打开确认层,
  已安装只显示提示。

## 基础包弹框

- 居中带边框标题,列出 Landscape 依赖的基础系统包
  (`pppd (ppp)`、`ip (iproute2)`、`iw (iw)`、`hostapd (hostapd)`、
  `sysctl (procps)`);
- 已安装的包显示 `✓` + 绿字"已安装",置灰不可切换(按 PATH 探测二进制);
- 缺失的包显示 `[x]`/`[ ]` 勾选态,默认全部勾选,Space/Enter 切换;
- 末行绿字动作项 "Install selected packages",选中 `FOCUS_SELECTED`;
- 底行灰字操作提示(Space 切换 / Enter 确认 / Esc 取消);
- Enter 在动作行提交:弹框关闭、记录选择并启动后台安装;Esc 关闭且还原
  打开前的选择。

## 基础包安装进度弹窗

- 居中带边框标题;安装提示行 + 弹窗内醒目的 `Esc 取消安装` 提示行(黄字加粗);
- 安装期间按 Esc 打开取消确认层:Enter 确认取消(置位标志终止正在运行的
  包管理器命令,已安装的包保留),Esc 关闭继续安装;
- 完成后刷新基础包状态,面板行恢复显示缺失数量或已安装;
- 软件包子进程设置 PDEATHSIG,退出控制台(Ctrl+C)后自动终止不留残留;

## 确认层

- 居中带边框标题(含软件名);
- 首行加粗问题;来源切换行:**蓝底白字加粗** `◀ 官方 ▶`,
  Space/←/→ 循环切换(官方仓库/阿里云/清华 TUNA/中科大 USTC);
- 切换提示行黄字;Enter 行白字、Esc 行灰字;
- Enter 确认、Esc 取消;确认后不退出 alternate screen。

## 安装进度弹窗

- 居中带边框标题;阶段文案(准备软件源 / 安装软件包 / 启动服务)+
  青色 Gauge + 弹窗内醒目的 `Esc 取消安装` 提示行(黄字加粗);
- 安装期间按 Esc 打开取消确认层:Enter 确认取消(置位标志终止
  正在运行的软件包管理器命令,已写入源文件保留、下次覆盖),
  Esc 关闭继续安装;取消后面板恢复,可重新选择来源;
- 软件包子进程设置 PDEATHSIG,退出控制台(Ctrl+C)后自动终止不留残留;

## 证据

- `console/software.rs` `render_software`、`render_software_confirmation`、
  `render_software_progress`、`render_software_cancel_confirmation`、
  `render_base_packages_dialog`、`render_base_packages_progress`;
- `software/base.rs` `run_command`(取消轮询 + PDEATHSIG);
- `software/docker.rs` `run_command`(取消轮询 + PDEATHSIG);
- 测试:`console/tests/software.rs`、`software/base.rs` 与 `software/docker.rs`
  的 `run_command_cancels_the_running_child_process`。
