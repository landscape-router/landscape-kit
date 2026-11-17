# `lkit reinit`

重新初始化已安装的 Landscape Router:备份当前配置 → 清理并重建数据 → 按首次安装
`--takeover-network` 的方式交互收集新的 WAN/LAN 网络计划 → 写入新凭据与新网络配置 →
重启 Landscape → 一律进入网络确认窗口,经 `lkit network confirm` 才提交。

```text
lkit [--non-interactive] reinit
     [--admin-user <NAME>] [--password-file <PATH>]
     [--allow-no-backup] [--yes]
```

## 适用范围(v1 硬性前置)

只接受已提交的安装,且必须同时满足:

- root 权限、真实可通信的服务管理器和支持原生 reinit 的运行中 daemon；
  CLI 在交互终端收集网络计划，TUI 将已确认的结构化计划直接提交；
- 安装状态存在且已提交;lkit 地盘无有效状态时返回参数错误并提示先执行
  `lkit install`;
- 服务后端为 systemd、OpenRC 或 SysVinit，并与安装状态一致。当前安装必须有同一
  canonical 根的已提交 install/reinit 网络接管事务作为依据；更新或修复不撤销该依据，
  后续已提交的普通安装、迁移或卸载使更早的接管记录失效。损坏或缺失的依据不能靠
  “某个网络服务已停止”推断补齐；未接管的安装不隐式接管；
- 逐一复核当前后端全部已安装宿主网络服务及 networkd 配套 socket、wait-online 实例。
  它们必须停止，systemd 保持 persistent mask，OpenRC/SysVinit 无开机链接；仅停用
  一个服务、临时 mask 或重新启用任一服务均不能通过。没有已安装宿主网络服务时，
  有效的已提交接管依据仍允许 reinit；
- 无未完成事务;存在待确认的网络接管事务时阻断并提示使用 `lkit network status`、
  `lkit network confirm` 或 `lkit network rollback`。

不接受的参数:`--version`、`--repository`、`--force`、`--takeover-network`。
reinit 不下载任何资产,版本固定为当前活动版本,release 与静态资产保持逐字节不变。

## 语义

reinit 是配置级重建,与 [restore](restore.md) 的数据库重建语义一致:

- 不复制 `landscape_db.sqlite` 字节文件;旧 `data/` 原子移动到事务目录,新数据目录按
  新生成的 `landscape_init.toml` 重建;
- 新 `landscape_init.toml` 的 `version` 固定为当前活动版本,受版本锁定约束;
- 新配置只包含登录凭据(用户重新输入)与用户选择的 WAN/LAN 网络实体(接口、IP 配置、
  firewall、LAN DHCP、route、WAN 管理端口静态映射);其余配置实体(DDNS、DNS 规则、
  已登记设备、证书、非网络 DHCP 之外的实体等)全部清空,由 Landscape 按新 init 配置
  重建数据库;
- 不恢复日志、指标历史、API token 与 Unix socket;新启动的 Landscape 重新生成证书等
  派生资产;
- `config.toml` 中的仓库来源记录不读取、不修改。

交互收集(凭据与网络计划)和破坏性计划确认必须先于任何修改完成:确认被拒绝或非交互
模式缺少 `--yes` 时不创建事务、不写任何文件、不停止服务。

服务定义必须包含 lkit 的启动恢复检查；缺少该检查时，先执行
[`lkit repair service`](repair.md#服务定义修复)，成功后重新执行 reinit。
reinit 不隐式改写服务定义。

## 确认窗口(一律进入)

服务健康检查通过后,reinit 不直接提交,而是进入 `awaiting_network_confirmation`:

- 事务记录主机启动标识与 10 分钟确认期限，由 daemon 监测和自动回滚；
- 提交 pending 安装状态,输出保护备份 ID、新管理地址与确认命令;
- 用户从任意可达主机(推荐重新连接到新管理地址)运行 `lkit network confirm` 复核接口
  MAC、管理地址、`br_lan` 成员、Landscape PID 与健康后提交;
- 期限内未确认、确认前重启或手工 `lkit network rollback` 走同一幂等回滚:停止服务 →
  恢复旧 `data/` → 恢复原服务运行状态与开机启动状态;回滚成功后事务标记 `rolled_back`,
  现场与保护 `.lkb` 保留;
- 确认或回滚成功后,后续 `lkit reinit` 才能再次运行。

无论新管理地址是否与旧地址相同,都执行确认窗口;这是 v1 的固定行为。

## 流程

1. 前端读取安装身份、校验前置条件；daemon 负责未完成事务恢复;
2. 交互收集目标配置:admin 用户与密码(规则同首次安装,缺失 `--password-file` 时通过
   `/dev/tty` 隐藏读取)与网络计划(接口发现与选择规则同
   [`--takeover-network`](install.md),见[网络重配置](../network/reinit.md));
3. 显示破坏性计划摘要并确认(非交互模式由 `--yes` 代替);
4. 提交绑定安装根、安装状态摘要、当前版本与网络计划的操作；daemon 取得安装锁后
   重新校验安装、接管依据和全部宿主服务停用状态，再创建保护 `.lkb`(`auto: true`,备注 `reinit 前自动备份`),必须完整落盘并自校验后才能
   停止服务;`--allow-no-backup` 才允许跳过并记录 `no_backup: true`;
5. 创建 `preparing` 事务并记录 `systemd_before`、`/etc/resolv.conf` 备份与状态快照;
6. 更新为 `stopping` 后停止服务并确认进程退出;
7. 更新为 `activating`:旧 `data/` 原子移动至事务目录 → 创建新空 `data/` → 写入新
   `landscape_init.toml`(权限 `0600`)→ 清理新选 LAN 接口地址(不检查、不协调
   `br_lan`,桥接由 Landscape 按新配置处理)→ 启动服务;
8. 更新为 `verifying`:完成 180 秒启动检查与 10 秒稳定观察;
9. 进入 `awaiting_network_confirmation`,输出确认命令与恢复机制说明;
10. `lkit network confirm` 完成校验并再次验证期限和启动标识后，先持久化 `finalizing`，
    再提交 state(`verified: true`)并更新为 `committed`;旧 data 事务现场与保护 `.lkb` 保留供诊断与人工 restore。

## 失败与回滚

- 目标激活或健康检查失败:停止服务 → 从事务目录恢复旧 `data/` → 恢复原服务状态;
  原本运行时启动旧配置并健康检查，原本停止时验证进程已退出并保持停止；
  原开机启动设置保持不变，`--allow-no-backup` 不改变该规则;
  回滚成功返回 `5`,回滚失败返回 `6` 并保留现场、输出人工恢复所需路径;
- 回滚优先使用事务目录中的旧 data 现场,不依赖保护 `.lkb`(后者用于人工 restore);
- 带完整网络恢复依据的中断 reinit 由 daemon 处理：进入确认窗口前回滚；
  `awaiting_network_confirmation` 在同次主机启动且未超时时等待，超时或重启则回滚；
  `finalizing` 续做提交，`rolling_back` 重试回滚。确认窗口内不因 daemon 进程重启
  提前回滚或延长期限。尚未建立完整恢复依据的准备阶段按通用事务清理规则处理。
- 确认健康检查跨过截止时刻时拒绝确认；完整判定见
  [daemon 网络接管恢复](../network/daemon-recovery.md#确认与恢复)。
- 退出码:成功 `0`,参数错误 `2`,普通失败 `1`,回滚成功 `5`,回滚失败 `6`,Ctrl+C `130`。

## 控制台

管理控制台侧栏提供 Reinit 面板：未安装、服务后端不受支持或缺少已提交接管依据时禁用。
渲染只读取持久化身份，不逐帧调用宿主服务管理器。提交前与 daemon 持锁执行时分别
复核全部宿主服务停用状态。面板复用网络向导与凭据表单,确认摘要后提交结构化操作，由 daemon 直接执行,
操作期间显示进度;进入 `awaiting_network_confirmation` 后显示待确认提示屏,与网络
接管一致(可内联运行 `lkit network confirm`)。

## 操作与凭据生命周期

前端确认之前不创建业务事务、不修改安装。请求绑定确认时的安装状态；领取时状态
或服务归属已变化则拒绝，不能把确认用于另一次安装。隐藏的控制台参数不能作为独立
CLI 绕过确认的凭据。daemon 不继承会话环境、不打开终端、不派生 lkit worker。

操作状态记录不包含密码。密码通过地盘内独立文件交接，目录 `0700`、文件 `0600`，
文件名由操作 ID 派生；排队取消时删除，daemon 领取后读取并删除。恢复只使用事务
快照与已生成配置，不重读密码、不重放请求。新配置和旧数据继续按事务恢复规则保存。

进入网络确认窗口时，操作为 `awaiting_confirmation`，CLI 返回 `0` 并提示确认命令，
这不代表已经提交。TUI 显示同一待确认状态，确认按钮绑定该事务 ID。
`operation status/list` 持续显示操作状态：确认提交后成功，超时或手工回滚后失败。
前端断开或 Ctrl+C 只退出观察；只允许取消尚未领取的请求。

控制台确认层绑定本次展示时的安装身份。Esc 取消会丢弃确认上下文，重新打开时读取
当前安装状态；确认层保持打开时不会自动改绑身份，提交时现场变化仍会被拒绝。
