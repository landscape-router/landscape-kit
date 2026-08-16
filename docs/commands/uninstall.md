# `lkit uninstall`

`uninstall` 卸载已安装的 Landscape Router:停止并注销 systemd 服务(或要求确认外部
实例已停止)、创建保护 `.lkb`、删除 landscape 安装根下的受管文件,并记录独立的
`uninstall` 事务。它只处理 `install-state.json` 有效且已提交的安装;未安装、状态损坏
或存在未完成事务时拒绝执行。lkit 自身(CLI 与常驻 daemon)不在卸载范围内,由
[`lkit self remove`](self.md#remove) 单独管理。

```text
lkit uninstall [--yes] [--allow-no-backup] [--keep-data]
```

landscape 安装根目录从 `.lkit/state/install-state.json` 读取(见
[安装布局与状态](../deployment/layout-and-state.md)),命令不接收 `--install-dir`。

`--non-interactive` 和 `--lang` 是全局参数。交互模式必须确认卸载计划、数据损失范围
(数据库、API token、日志和指标不可逆删除)与保留物;非交互模式必须额外提供 `--yes`,
否则直接返回参数错误(`2`)。`--yes` 覆盖全部确认:卸载计划、minimal scope 数据损失、
网络接管警告。

从交互控制台(TUI)发起的 uninstall 由 TUI 卸载确认层完成全部确认,命令内部标记
`--console-confirmed`,不再请求 `/dev/tty` 二次确认;这在 systemd worker 路径下是必需
的——worker 是独立进程且无法读取 TUI 的键盘输入。控制台的卸载入口当前暂未启用
(见[管理控制台](../interaction/console.md)),卸载只经命令模式使用;`--console-confirmed`
参数与面板代码完整保留,重新启用后行为与本段描述一致。

确认发生在事务创建之前:用户拒绝或缺少 `--yes` 时不创建事务、不写任何文件,服务与
现场保持不变,分别返回 `1` 和 `2`。

## 清理选项

默认卸载删除 landscape 安装根下的全部受管内容:

- `current` 软链接与全部 `releases/<version>` 目录;
- `data/`(Landscape home path:数据库、初始化文件、持久配置、日志、metric);
- `service/` 受管服务定义原件。

lkit 地盘(`/root/.lkit/`)原样保留:

- `config.toml` 由用户维护,`lkit` 在任何模式下都不创建、更新或删除它,见
  [配置文件](../deployment/config.md);
- `backups/` 与 `transactions/` 是卸载的恢复现场与保护备份存放点,默认保留;输出会提示
  用户取走 `.lkb` 并确认删除后自行清理;
- `logs/` 与 `run/`(含 `install.lock`)同样保留,不属于 landscape 卸载范围。

`--keep-data`:只卸载服务与程序,保留 landscape 根的 `data/`(含 `landscape_init.toml`
与数据库)和 `config.toml`,删除其余全部受管内容与 `current`。安装视为已卸载,
`install-state.json` 被删除;保留的 `data/` 不再属于受管现场,用户需自行处理(如拷贝
后删除),再次执行 `lkit install` 属于全新首次安装。

不存在 `--purge-root`:lkit 地盘不属于卸载范围,整树删除不被支持。

## 卸载前检查

卸载只接受已有有效 `install-state.json` 的安装。在停止服务前完成:

1. 解析 lkit 地盘与 landscape 根目录,获取安装锁,恢复未完成事务(见[中断恢复](#中断恢复));
2. 读取 `install-state.json`;不存在时返回参数错误 `2` 并提示先执行 `lkit install`;
3. 校验受管 unit 所有权与后端摘要(与 `switch` 相同的安全不变量,失败阻断);
4. 交互确认卸载计划与数据损失范围(非交互模式以 `--yes` 代替);检测到网络接管特征
   (NetworkManager、ifupdown 的 `networking.service`、firewalld 或 systemd-resolved 被
   停止、disable 或 mask)时追加醒目 warning:卸载不会恢复宿主网络服务,需要用户自行
   恢复。该警告可被确认,不阻断;
5. 默认创建保护 `.lkb`(固定备注 `uninstall 前自动保护备份`,auto 标记为 true),失败
   默认阻断;`--allow-no-backup` 显式跳过,并明确表示不产生可移植的当前配置快照;
6. 创建 `uninstall` 事务,记录 `systemd_before`、`previous_current`、`backup` 引用和
   (systemd 环境)必要的 `/etc/resolv.conf` 备份现场。

保护备份创建失败、所有权冲突或状态损坏时,保持当前服务和现场不变。
通过 `/dev/tty` 确认外部实例已停止(非交互模式以 `--yes` 代替),`lkit` 不启动、不探测
外部进程。

lkit 常驻 daemon(若已安装)继续运行,不参与卸载;卸载后的空闲恢复循环只扫描 lkit
地盘,不存在未完成事务时无任何动作。需要移除 daemon 时另行执行 `lkit self remove`。

## 执行与提交

由常驻 daemon 委托执行(与 switch/restore 相同的委托边界),并按
以下顺序执行:

1. 将事务标记为 `stopping`,停止受管服务并确认进程退出;
2. 将事务标记为 `activating`,`disable` 并注销 systemd 注册链接,执行
   `daemon-reload`;
3. 按上述清理选项删除受管内容(保留物见[清理选项](#清理选项));
4. 将事务标记为 `committed`,输出卸载结果、保护备份 ID 与保留物清单。

systemd 模式停止、disable 并注销受管的 Landscape 服务后删除受管内容;文件删除与事务
提交直接进行,输出与 systemd 模式相同。none 模式要求交互确认外部实例已停止
(非交互以 `--yes` 代替)。

卸载成功后 `install-state.json` 不再存在,再次运行 `lkit install` 按全新首次安装处理。

## 中断恢复

卸载是用户明确请求的终态,中断恢复采用**前向完成**语义,不自动回滚:

- `preparing`:尚未改变运行状态,清理临时文件并标记 `failed`(用户可重新执行卸载);
- `prepared` 或 `stopping`:服务可能已停止,继续完成注销、文件删除与提交;
- `activating`:继续完成文件删除与提交;
- 恢复再次失败时标记 `failed`,保留可用的保护 `.lkb`、事务现场和失败现场,不无限循环
  重试,要求人工诊断。

保护 `.lkb` 与事务现场存放在 lkit 地盘,保证失败后仍可人工恢复配置快照。已提交的
卸载不提供回滚入口;恢复配置只能使用卸载前创建的 `.lkb` 重新安装。

## 失败语义

- 参数错误返回 `2`(含非交互模式缺少 `--yes`);
- 用户拒绝、确认失败、保护备份失败、锁冲突或删除失败返回普通失败 `1`,事务标记
  `failed`,现场保留;
- 显式 Ctrl+C 返回 `130`;systemd worker 被停止时输出 warning 并保留现场。

卸载没有自动回滚语义,因此不定义退出码 `5` 和 `6`。输出与退出码的全局约定见
[输出与退出码](output-and-exit-codes.md)。
