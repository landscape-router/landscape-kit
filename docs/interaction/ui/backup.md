# Backup 面板样式验收标准

- 来源:侧栏面板;未安装/非 root 时只显示原因提示
- 对应实现:`console/backup/render.rs`、`console/backup/keys.rs`

## 不可用状态

- RootRequired:黄字"需要 root 权限" + 灰字"备份需要已安装 Landscape";
- NotInstalled/Unavailable:黄字原因 + 灰字提示。

## 备份列表

- 顶部"创建备份"动作行:选中 `FOCUS_SELECTED` 反色 + `> ` 标记;
  未选中 `ACTION_HINT` 绿字 + 加粗;
- 条目(**备注排第一**,后跟 `backup_id + created_at + landscape_version`):
  - 选中 `FOCUS_SELECTED` + `> ` 标记;未选中默认色;
  - **单行展示:其他信息固定占位,备注按剩余长度截断为省略号(不换行)**,
    完整备注进详情页;
  - invalid 条目红字 + invalid 徽标;
- 加载中灰字、失败红字、无备份灰字。

## 详情页(V/Enter 打开)

- 标题加粗 + panel_block 焦点边框,Up/Down 滚动,可换行;
- 字段顺序:**备注第一**,其后 backup_id / 创建时间 / Landscape 版本 /
  lkit 版本 / 架构 / 主机名 / 是否自动 / scope / contents;
- 底部灰字恢复提示行(R 恢复、V 校验);
- 校验行为:
  - 进入详情即**自动**在后台执行完整校验(读文件 + `verify_lkb` + 解包),
    结果写底栏,不阻塞查看;V 键保留,可随时手动重校验;
  - 校验失败(备份损坏)时,R 键打开损坏提示弹框,不进入恢复确认层。

## 损坏提示弹框

- 居中带边框标题 `Backup corrupt`;
- 标题行黑底黄字加粗 + 灰字说明(完整校验未通过,无法恢复);
- Enter/Esc 关闭,不触发任何动作。

## 恢复确认层

- 居中带边框标题 `Confirm restore`;首行加粗问题、计划行、
  灰字 Esc 取消行;
- 行为:Enter 前校验必须通过(未校验先启动校验并提示"校验中",
  校验失败弹损坏框),通过才提交带 `--console-confirmed` 的 `Restore` 请求;

## 创建对话框(备注输入)

- 居中带边框标题 `Create backup`;minimal scope 说明行;
- 备注行:标签加粗、输入值下划线样式 + 光标 `_`,最多 256 字符;

## 创建进度弹窗

- 居中带边框标题;阶段文案(导出配置 / 归档 N/M / 落盘校验)+ 青色 Gauge +
  灰字提示行;弹窗外=Esc。

## 删除确认层

- 同恢复确认层布局(标题 `Confirm delete`,展示备份 ID 与版本、
  "将永久删除"提示);Enter 删除、Esc 取消;删除在控制台内同步执行。

## 证据

- 列表与详情:`console/backup/render.rs`;
- 校验与按键:`console/backup/keys.rs`、`console/backup/mod.rs`;
- 测试:`console/tests/backup.rs`(`opening_details_starts_automatic_verify`、
  `restore_enter_verifies_before_submitting`、
  `restore_enter_rejects_when_verify_failed_and_dialog_closes` 等)。
