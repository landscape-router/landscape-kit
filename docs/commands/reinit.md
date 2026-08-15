# `lkit reinit`

重新初始化已安装的 Landscape Router:备份当前配置 → 清理并重建数据 → 按首次安装
`--takeover-network` 的方式交互收集新的 WAN/LAN 网络计划 → 写入新凭据与新网络配置 →
重启 Landscape → 一律进入网络确认窗口,经 `lkit network confirm` 才提交。

```text
lkit [--non-interactive] reinit [--install-dir <PATH>]
             [--admin-user <NAME>] [--password-file <PATH>]
             [--allow-no-backup] [--yes]
```

## 适用范围(v1 硬性前置)

只接受已提交的安装,且必须同时满足:

- root 权限、真实可通信的服务管理器和交互终端(网络收集必须交互;控制台委托的内部
  daemon worker 通过隐藏 `--network-plan-file` 提供计划,见 [daemon worker](../service/runtime-and-health.md));
- 安装状态存在且已提交;目标目录中不存在有效状态时返回参数错误并提示先执行
  `lkit install`;
- `service.manager == systemd`,且宿主网络服务已被接管(NetworkManager、
  `networking.service`、firewalld、systemd-resolved 处于 stop/disable/mask 状态);
  未接管的安装提示不支持,不回退、不隐式接管;
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

## 确认窗口(一律进入)

服务健康检查通过后,reinit 不直接提交,而是进入 `awaiting_network_confirmation`:

- lkit 将自身复制为 root-only 恢复二进制,并安装 10 分钟确认期限的 persistent timer、
  timer 调用的幂等 rollback service 与未确认重启时的 boot rollback service;
- 提交 pending 安装状态,输出保护备份 ID、新管理地址与确认命令;
- 用户从任意可达主机(推荐重新连接到新管理地址)运行 `lkit network confirm` 复核接口
  MAC、管理地址、`br_lan` 成员、Landscape PID 与健康后提交;
- 期限内未确认、确认前重启或手工 `lkit network rollback` 走同一幂等回滚:停止服务 →
  恢复旧 `data/` → 重启旧配置并通过健康检查;回滚成功后事务标记 `rolled_back`,
  现场与保护 `.lkb` 保留;
- 确认或回滚成功后,后续 `lkit reinit` 才能再次运行。

无论新管理地址是否与旧地址相同,都执行确认窗口;这是 v1 的固定行为。

## 流程

1. 校验前置条件、获取安装锁并恢复未完成事务;
2. 交互收集目标配置:admin 用户与密码(规则同首次安装,缺失 `--password-file` 时通过
   `/dev/tty` 隐藏读取)与网络计划(接口发现与选择规则同
   [`--takeover-network`](install.md),见[网络重配置](../network/reinit.md));
3. 显示破坏性计划摘要并确认(非交互模式由 `--yes` 代替);
4. 创建保护 `.lkb`(`auto: true`,备注 `reinit 前自动备份`),必须完整落盘并自校验后才能
   停止服务;`--allow-no-backup` 才允许跳过并记录 `no_backup: true`;
5. 创建 `preparing` 事务并记录 `systemd_before`、`/etc/resolv.conf` 备份与状态快照;
6. 更新为 `stopping` 后停止服务并确认进程退出;
7. 更新为 `activating`:旧 `data/` 原子移动至事务目录 → 创建新空 `data/` → 写入新
   `landscape_init.toml`(权限 `0600`)→ 清理新选 LAN 接口地址(不检查、不协调
   `br_lan`,桥接由 Landscape 按新配置处理)→ 启动服务;
8. 更新为 `verifying`:完成 180 秒启动检查与 10 秒稳定观察;
9. 进入 `awaiting_network_confirmation`,输出确认命令与恢复机制说明;
10. `lkit network confirm` 通过后提交 state(`verified: true`)、更新为 `finalizing` 再
    更新为 `committed`;旧 data 事务现场与保护 `.lkb` 保留供诊断与人工 restore。

## 失败与回滚

- 目标激活或健康检查失败:停止服务 → 从事务目录恢复旧 `data/` → 重启旧配置并健康检查;
  回滚成功返回 `5`,回滚失败返回 `6` 并保留现场、输出人工恢复所需路径;
- 回滚优先使用事务目录中的旧 data 现场,不依赖保护 `.lkb`(后者用于人工 restore);
- 中断恢复规则见[事务与中断恢复](../deployment/transactions-and-recovery.md):
  `preparing` 标记 `failed`;`prepared`/`stopping` 恢复事务前 systemd 状态后标记
  `failed`;`activating`/`verifying` 执行上述回滚;
  `awaiting_network_confirmation`/`finalizing`/`rolling_back` 阻断并提示使用
  `lkit network confirm` 或 `lkit network rollback`;
- 退出码:成功 `0`,参数错误 `2`,普通失败 `1`,回滚成功 `5`,回滚失败 `6`,Ctrl+C `130`。

## 控制台

管理控制台侧栏提供 Reinit 面板,入口与 CLI 等价:未安装、非 systemd 或宿主网络服务
未接管时禁用。面板复用网络向导与凭据表单,确认摘要后由临时 systemd worker 执行,
操作期间显示进度;进入 `awaiting_network_confirmation` 后显示待确认提示屏,与网络
接管一致(可内联运行 `lkit network confirm`)。
