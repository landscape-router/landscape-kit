# 安装、切换与修复生命周期

## 安装与版本切换流程

### 1. 准备

1. 验证 root、平台和架构；
2. 解析并固定安装根目录真实路径；
3. 获取 `run/install.lock` 非阻塞独占锁；
4. 读取安装状态和未完成事务；
5. 检测危险目录、旧部署、进程和 systemd unit 冲突；
6. 调用部署前检查；
7. 解析仓库和目标版本；
8. 创建 `preparing` 事务。

无法获取安装锁时立即失败，说明另一个安装过程可能正在运行。

读取状态后如果确认请求的是 service manager 迁移，则转入“Service manager 迁移”流程，不继续执行本节的仓库解析、目标资产准备、`.lkb` 备份或版本激活步骤。迁移仍复用适用的平台、路径、状态、事务、受管文件和安全检查；`none → systemd` 的端口检查固定在用户确认外部实例已停止之后执行。

### 2. 准备目标资产

1. 下载后端和静态压缩包；
2. 校验大小、SHA-256、架构和压缩包安全；
3. 构造完整目标版本目录；
4. 按 [`repository.md`](../repository.md) 的“下载与发布目录”规则复用可信已有目录或阻断不可信目录；
5. 在停止当前服务前完成所有网络下载。

### 3. 首次安装初始化

首次安装的共同准备阶段固定如下：

1. 创建 `preparing` 事务后，创建目录布局；
2. 获取并验证管理员凭据；
3. 创建 `data/landscape_init.toml`，权限 `0600`；
4. systemd 环境记录 `systemd_before`，备份 `/etc/resolv.conf` 并写入 `resolv_conf_backup`；无 systemd 环境两者记录为 null；
5. systemd 环境准备 unit，无 systemd 环境只生成供用户参考的启动命令；
6. 确认目标版本目录、初始化文件以及当前环境所需的其他文件均已完整落盘后，将事务从 `preparing` 更新为 `prepared`；
7. 在第一次修改 `current` 或 systemd 注册链接前，将事务更新为 `activating`；
8. 原子创建 `current`。

systemd 环境继续执行：

9. 注册、启用并启动服务；
10. 目标进程成功创建后、开始健康检查前，将事务更新为 `verifying`；
11. 完成 180 秒启动检查和 10 秒稳定观察；
12. 记录 `initialization.status: complete`、初始化锁状态和 `service.verified: true`；
13. 原子提交 `install-state.json`；
14. 状态文件提交成功后，将事务更新为 `committed`。

无 systemd 环境不进入 `verifying`，而是继续执行：

9. 记录 `initialization.status: pending`、`lock_present: false`、`initialized_at: null` 和 `service.verified: false`；
10. 原子提交 `install-state.json`；
11. 状态文件提交成功后，将事务更新为 `committed`；
12. 输出参考启动命令，但不执行、不等待也不检查。

首次安装在 `activating` 或 systemd 的 `verifying` 阶段失败或中断时没有 `.lkb`，必须执行首次安装失败清理，不得进入配置级回滚。

使用 `--takeover-network` 时，健康检查通过后先写入 pending install state 并进入
`awaiting_network_confirmation`；此时安装尚未提交。只有运行 `lkit network
confirm`（不限制 SSH 会话来源）通过复检才能提交状态。10 分钟 timer、确认前重启的 boot
rollback 和手工
`lkit network rollback` 都进入同一回滚路径；回滚成功后删除整个未提交 `data/`，恢复宿主
网络，并回到可重新首次安装的状态。

### 4. Repair 阶段转换

需要替换后端的 repair 使用以下阶段边界：

1. `preparing`：下载并校验原始可信后端，导出配置并创建 `.lkb`；
2. `.lkb`、修复后端和修复前二进制完整落盘；systemd 环境还必须记录 `systemd_before`，完成 `/etc/resolv.conf` 备份并写入 `resolv_conf_backup`，然后更新为 `prepared`；
3. systemd 环境在调用 stop 前先更新为 `stopping`，再由 `lkit` 停止服务；无 systemd 环境要求用户自行停止并明确确认，`lkit` 不验证运行态；
4. 第一次替换后端前更新为 `activating`；
5. systemd 环境启动修复后进程，并在健康检查前更新为 `verifying`；健康检查成功并提交状态后更新为 `committed`；
6. systemd 环境启动失败时更新为 `rolling_back`，使用 `.lkb` 和修复前二进制恢复；恢复成功后更新为 `rolled_back`；
7. 无 systemd 环境完成原子文件替换后直接提交 `verified: false` 并更新为 `committed`，不启动、不检查，也不因用户之后启动失败自动回滚。

`lkit repair static` 不停止 Landscape，也不创建 `.lkb`：

- `preparing` 时下载发布静态包并备份当前 `static/`；
- 备份和目标静态目录准备完成后更新为 `prepared`；
- 替换静态目录前更新为 `activating`；
- 静态文件由运行中的 Landscape 热加载，替换完成后直接提交状态，不执行 `/api/docs` 探测或任何运行态检查；
- 替换失败时恢复原静态目录，更新为 `rolled_back`；恢复失败则为 `failed`。

### 5. 版本切换准备

1. 按 SemVer 比较当前活动版本和目标版本。目标版本更低时，在创建切换事务和下载目标
   二进制、静态资产前拒绝；目标版本相同时转入同版本安装校验，只有更高版本可以继续；
2. 验证当前后端摘要和 systemd 服务状态；
3. 服务正在运行时，调用配置导出 API，创建包含当前二进制、静态页面、导出配置和
   `geo_tmp` 的 `.lkb`，并完整自校验；即使指定 `--allow-no-backup` 也不得跳过；
4. systemd 服务已停止时，默认在创建事务前拒绝。仅显式 `--allow-no-backup` 时跳过导出
   和 `.lkb`，并在事务中记录 `no_backup: true` 且不记录 `backup`；
5. systemd 环境备份 `/etc/resolv.conf`；无 systemd 环境不创建该主机状态备份；
6. 记录 `previous_current`、可选备份和目标目录；
7. 将事务标记为 `prepared`。

需要备份时，在 `.lkb` 完整落盘并自校验前不得停止当前 Landscape。无备份例外只适用于
已经停止的 systemd 服务，不适用于后端 repair 或无 systemd 环境。

### 6. 激活

systemd 环境：

1. 将事务标记为 `stopping`；
2. 停止当前服务并确认进程退出；
3. 将事务标记为 `activating`；
4. 使用临时软链接和原子 rename 更新 `current`；
5. 更新受管服务定义；
6. 执行 systemd reload/enable/start；
7. 将事务标记为 `verifying`；
8. 执行 180 秒启动检查和 10 秒稳定观察。

无 systemd 环境：

1. 在前述人工检查点由用户明确确认已停止当前实例；`lkit` 不检查 PID、端口或实际停止状态；
2. 将事务标记为 `activating`；
3. 使用临时软链接和原子 rename 更新 `current`，或原子替换 repair 后端；
4. 不启动目标版本，不进入运行态 `verifying` 健康检查；
5. 文件和状态写入校验通过后直接进入成功提交。

不得先删除旧 `current` 再创建新链接。

### 7. 成功提交

1. 原子写入新的 `install-state.json`；systemd 环境记录 `verified: true`，无 systemd 环境记录 `verified: false`；
2. 保留旧版本和 `.lkb`；
3. 将事务标记为 `committed`；
4. 释放安装锁；
5. 输出版本、目录、服务状态、管理地址、备份 ID 和主机 warning。

成功状态必须最后提交。

上述会改变 systemd 或 Landscape 运行态的生产路径在进入流程前已经由临时 operation
unit 托管；CLI 等待进程退出不会中止事务 worker。主机重启后的处理仍依赖下次调用
执行阶段恢复。

### 8. 手工备份与恢复

`lkit backup create` 是不改变运行态的只读管理操作：取得安装锁后读取当前 state、校验
运行实例和路径，通过配置导出 API 取得 TOML，再按 `.lkb` v1 minimal 规则流式生成并自校验
最终文件。它不创建业务事务，不停止服务，也不更新 `install-state.json`。

`lkit restore` 使用独立的 `restore` 事务：

1. 在 `preparing` 验证目标 `.lkb`、架构和归档内容；
2. 默认创建当前实例的保护 `.lkb`，并记录目标备份和保护备份引用；
3. 保存 `previous_current`、previous-state、systemd 事实和必要的 `/etc/resolv.conf`；
4. 在 `prepared` 后进入 `stopping`，将旧 `data/` 原子移动到事务目录；
5. 在 `activating` 从目标备份重建版本目录、空 data、初始化配置和 Geo 缓存；
6. systemd 模式进入 `verifying`，通过启动和稳定健康检查后写入目标 state；none 模式直接
   提交 pending/未验证状态并输出参考启动命令；state 的 `static_archive` 身份从备份内
   压缩包现场计算，仓库来源记录（`config.toml`）保持不变；
7. 成功提交后保留旧 data 事务现场和备份，事务进入 `committed`。

目标激活或健康检查失败时进入 `rolling_back`，优先用事务目录中的旧 data 和 state 恢复，
必要时使用保护 `.lkb`；恢复成功进入 `rolled_back` 并返回 `5`，恢复失败进入 `failed`
并返回 `6`。`--allow-no-backup` 只跳过保护 `.lkb`，不跳过目标校验、确认或中断恢复事实。

### 9. 卸载

`lkit uninstall` 是独立的 `uninstall` 事务,只处理已提交安装,不负责空目录或状态损坏
现场。完整命令规格见 [`lkit uninstall`](../commands/uninstall.md)。

1. 获取安装锁并恢复未完成事务;读取已提交 state,不存在则参数错误;
2. 交互确认卸载计划与数据损失范围(非交互模式以 `--yes` 代替);网络接管特征警告
   (宿主网络服务被 stop/disable/mask)只提示不阻断;
3. 默认创建保护 `.lkb`(`uninstall 前自动保护备份`,auto 标记为 true),失败阻断;
   `--allow-no-backup` 显式跳过;
4. 创建 `preparing` 卸载事务,记录 `systemd_before`、`previous_current`、备份引用和
   systemd 环境必要的 `/etc/resolv.conf` 备份现场;
5. systemd 环境更新为 `stopping` 后停止服务并确认进程退出,再更新为 `activating`,
   执行 `disable`、注销注册链接和 `daemon-reload`;none 环境要求用户确认外部实例已停
   止,`lkit` 不验证运行态;
6. 删除受管内容。默认保留 `config.toml`、`backups/` 与 `transactions/`;`--keep-data`
   额外保留 `data/`;`--purge-root` 整树删除安装根目录(必须同时给出
   `--allow-no-backup`);
7. 提交 `committed`,输出卸载结果、保护备份 ID 与保留物清单。

卸载中断恢复采用前向完成:继续注销服务、删除文件并提交,不自动回滚;恢复再次失败
标记 `failed` 并保留现场要求人工诊断。卸载成功后该根目录视为全新首次安装。

### 10. 重新初始化

`lkit reinit` 是独立的 `reinit` 事务,只接受已提交、systemd 且宿主网络服务已被接管的
安装,配置级重建语义与 restore 一致(不复制数据库字节文件,按新 init 配置重建)。
完整命令规格见 [`lkit reinit`](../commands/reinit.md),网络细节见
[网络重配置](../network/reinit.md)。

1. 获取安装锁并恢复未完成事务;读取已提交 state,不存在则参数错误;校验 systemd 与
   宿主网络服务已接管;
2. 交互收集目标配置(凭据 + 网络计划)并确认破坏性计划,非交互模式以 `--yes` 代替;
   收集与确认先于任何修改,拒绝时不创建事务、不写文件;
3. 默认创建保护 `.lkb`(`reinit 前自动备份`,auto 标记为 true),失败阻断;
   `--allow-no-backup` 显式跳过;
4. 创建 `preparing` reinit 事务,记录 `systemd_before`、`/etc/resolv.conf` 备份、状态
   快照与网络计划;`version` 固定为当前活动版本,不下载任何资产;
5. 更新为 `stopping` 后停止服务并确认进程退出;
6. 更新为 `activating`:旧 `data/` 原子移动到事务目录,创建新空 `data/` 并写入新
   `landscape_init.toml`(权限 `0600`,版本等于当前活动版本,凭据为用户新输入,网络为
   新计划),清理新选 LAN 接口地址;不检查、不协调 `br_lan`,桥接创建、成员同步与
   清理全部由 Landscape 按新配置处理;
7. 更新为 `verifying`:启动服务并完成 180 秒启动检查与 10 秒稳定观察;
8. 一律进入 `awaiting_network_confirmation`:arm 恢复二进制、确认 timer 与 boot
   rollback,写入 pending state;`lkit network confirm` 复核接口 MAC、管理地址、
   `br_lan` 成员、PID 与健康后提交;
9. 成功提交后保留旧 data 事务现场与保护备份,事务进入 `committed`,输出新管理地址与
   备份 ID。

失败语义:目标激活或健康检查失败时进入 `rolling_back`,从事务目录恢复旧 `data/` 并
重启旧配置,回滚成功返回 `5`、失败返回 `6`;timer 到期、确认前重启与手工 rollback
走同一回滚入口。中断恢复:`preparing` 标记 `failed`;`prepared`/`stopping` 恢复事务前
systemd 状态后标记 `failed`;`activating`/`verifying` 执行旧 data 回滚;
`awaiting_network_confirmation`/`finalizing`/`rolling_back` 阻断并提示使用
`lkit network confirm` 或 `lkit network rollback`。

### 11. 手工部署迁移

`lkit migrate` 把手工部署（非 lkit 安装格式）迁移为受管安装，是独立的 `migrate`
事务，目标安装根必须全新。完整命令规格见 [`lkit migrate`](../commands/migrate.md)。

1. 获取安装锁并恢复未完成事务；校验安装根无状态且无遗留受管内容；校验 `--from`
   目录含 Landscape 特征文件且不是受管安装的 data；
2. 按固定端口识别运行中的旧实例（`--config-dir` 匹配源目录），未运行则报错提示先启动；
   端口上有无法确认身份的进程时阻断；
3. 创建 `preparing` 迁移事务，通过导出 API 读取当前配置与后端版本（备份不升级），
   从 `/proc/<pid>/exe` 读取二进制，static 目录取进程 `--web` 参数或缺省
   `<from>/static`，`static.zip` 本地缺失时从发布仓库下载、仓库不可用时从现场打包，
   生成迁移 `.lkb` 到 `backups/`；
4. 交互确认迁移计划（非交互必须 `--yes`），拒绝时不创建事务、不写文件；
5. 更新为 `prepared`，再更新为 `stopping`：systemd 可用时按 `ExecStart --config-dir`
   匹配发现旧 unit——唯一匹配则 stop/disable（原件位于 `/etc/systemd/system` 时移入
   事务目录，否则 mask），无匹配或多匹配分别要求人工确认或先清理；旧实例进程仍存活
   时同样要求确认；
6. 更新为 `activating`：从迁移备份解包重建 `releases/<版本>`，创建空 `data/`、写入
   导出的 `landscape_init.toml`（`0600`）、恢复 `geo_tmp`，原子创建 `current`；
7. systemd 模式更新为 `verifying`：写入受管 unit 原件、注册、启用、启动并完成完整
   健康检查；none 模式不启动不检查，提交 pending/未验证状态并输出参考启动命令；
8. 提交 state（版本为被迁移版本、资产身份来自备份、systemd 模式
   `initialization.status: complete`），事务进入 `committed`，输出迁移备份 ID 与旧部署
   清理提示。

失败语义：停止旧实例前失败标记 `failed` 返回 `1`；停止后失败进入 `rolling_back`——
注销/停止新受管 unit、恢复 `/etc/resolv.conf`、恢复旧 unit（文件放回原位或 unmask，
按 enabled/active 重启）、删除新根内容，回滚成功返回 `5`、失败返回 `6`；前台实例
场景不自动重启旧实例。中断恢复见
[事务与中断恢复](../deployment/transactions-and-recovery.md)。

## 首次安装失败

systemd 环境首次安装启动失败时：
- 停止失败服务；
- 恢复或移除本次创建的 `current`；
- 按事务中的 `systemd_before` 恢复 systemd 注册链接以及 enabled/active 状态并 reload；
- 按事务中的 `resolv_conf_backup` 恢复 `/etc/resolv.conf` 原始状态；
- 不写入成功状态；
- 保留必要的 root-only 日志和事务信息；
- 将事务标记为 `failed`。

不得留下已启用但无法启动的 Landscape 服务。
