# 安装、切换与修复生命周期

## 安装与版本切换流程

### 1. 准备

1. 验证 root、平台和架构；
2. 解析并固定 lkit 地盘与 landscape 安装根真实路径；
3. 获取 `/root/.lkit/run/install.lock` 非阻塞独占锁；
4. 读取安装状态和未完成事务；
5. 检测危险目录、旧部署、进程和 systemd unit 冲突；
6. 调用部署前检查；
7. 解析仓库和目标版本；
8. 创建 `preparing` 事务。

无法获取安装锁时立即失败，说明另一个安装过程可能正在运行。

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
4. 记录 `systemd_before`，备份 `/etc/resolv.conf` 并写入 `resolv_conf_backup`；
5. 准备 unit；
6. 确认目标版本目录、初始化文件以及当前环境所需的其他文件均已完整落盘后，将事务从 `preparing` 更新为 `prepared`；
7. 在第一次修改 `current` 或 systemd 注册链接前，将事务更新为 `activating`；
8. 原子创建 `current`。

继续执行：

9. 注册、启用并启动服务；
10. 目标进程成功创建后、开始健康检查前，将事务更新为 `verifying`；
11. 完成 180 秒启动检查和 10 秒稳定观察；
12. 记录 `initialization.status: complete`、初始化锁状态和 `service.verified: true`；
13. 原子提交 `install-state.json`；
14. 状态文件提交成功后，将事务更新为 `committed`。

首次安装在 `activating` 或 `verifying` 阶段失败或中断时没有 `.lkb`，必须执行首次安装失败清理，不得进入配置级回滚。

使用 `--takeover-network` 时，健康检查通过后先写入 pending install state 并进入
`awaiting_network_confirmation`；此时安装尚未提交。只有运行 `lkit network
confirm`（不限制 SSH 会话来源）通过复检才能提交状态。daemon 按 10 分钟确认期限和
主机启动标识执行自动回滚，手工
`lkit network rollback` 都进入同一回滚路径；回滚成功后删除整个未提交 `data/`，恢复宿主
网络，并回到可重新首次安装的状态。

### 4. Repair 阶段转换

repair 使用[daemon 操作管理](../service/daemon.md)的结构化请求与持久化结果。
操作与事务的关联先于事务创建；操作阶段独立于事务阶段。下载取消窗口在导出、备份
和任何活动资产修改前关闭。恢复不重新提交工作流，也不重新解析资产来源。

需要替换后端的 repair 按安装记录的服务管理后端使用以下阶段边界：

1. `preparing`：下载并校验原始可信后端，导出配置并创建 `.lkb`；
2. `.lkb`、修复后端和修复前二进制完整落盘；记录 `systemd_before`，完成 `/etc/resolv.conf` 备份并写入 `resolv_conf_backup`，然后更新为 `prepared`；
3. 在调用 stop 前先更新为 `stopping`，再由 `lkit` 停止服务；
4. 第一次替换后端前更新为 `activating`；
5. 启动修复后进程，并在健康检查前更新为 `verifying`；健康检查成功并提交状态后更新为 `committed`；
6. 启动失败时更新为 `rolling_back`，使用 `.lkb` 和修复前二进制恢复；恢复成功后更新为 `rolled_back`。

`lkit repair static` 不停止 Landscape，也不创建 `.lkb`：

- `preparing` 时下载并校验选定来源的静态包；结束下载取消窗口后，备份当前
  `static/`、版本目录中的 `static.zip`（或记录不存在）和安装状态；
- 备份和目标静态目录准备完成后更新为 `prepared`；
- 修改版本归档或替换静态目录前更新为 `activating`，不在备份前刷新 `static.zip`；
- 静态文件由运行中的 Landscape 热加载，替换完成后直接提交状态，不执行 `/api/docs` 探测或任何运行态检查；
- 提交前失败或中断时恢复静态目录、版本归档和旧安装状态，更新为 `rolled_back`；
  恢复失败保留失败现场和阻断依据，不能只恢复其中一项后报告完成。

### 5. 版本切换准备

1. 按 SemVer 比较当前活动版本和目标版本。目标版本更低时，在创建切换事务和下载目标
   二进制、静态资产前拒绝；目标版本相同时转入同版本安装校验，只有更高版本可以继续；
2. 验证当前后端摘要和安装记录中后端的服务状态；
3. 服务正在运行时，调用配置导出 API，创建包含当前二进制、静态页面、导出配置和
   `geo_tmp` 的 `.lkb`，并完整自校验；即使指定 `--allow-no-backup` 也不得跳过；
4. 受管服务已停止时，默认在创建事务前拒绝。仅显式 `--allow-no-backup` 时跳过导出
   和 `.lkb`，并在事务中记录 `no_backup: true` 且不记录 `backup`；
5. 备份 `/etc/resolv.conf`；
6. 记录 `previous_current`、可选备份和目标目录；
7. daemon switch 无论是否有 `.lkb` 都持久化旧安装状态；目标资产先在事务目录内准备，下载取消窗口结束后才能进入备份和停止阶段；
8. 将事务标记为 `prepared`。

需要备份时，在 `.lkb` 完整落盘并自校验前不得停止当前 Landscape。无备份例外只适用于
已经停止的受管服务，不适用于后端 repair。

### 6. 激活

1. 将事务标记为 `stopping`；
2. 停止当前服务并确认进程退出；
3. 将事务标记为 `activating`；
4. 使用临时软链接和原子 rename 更新 `current`；
5. 更新受管服务定义；
6. 执行 systemd reload/enable/start；
7. 将事务标记为 `verifying`；
8. 执行 180 秒启动检查和 10 秒稳定观察。

不得先删除旧 `current` 再创建新链接。

### 7. 成功提交

1. 原子写入新的 `install-state.json`；记录 `verified: true`；
2. 保留旧版本和 `.lkb`；
3. 将事务标记为 `committed`；
4. 释放安装锁；
5. 输出版本、目录、服务状态、管理地址、备份 ID 和主机 warning。

成功状态必须最后提交。

生产安装、重初始化、迁移、切换、修复、恢复、卸载和网络确认／回滚均由 daemon 直接执行结构化操作；前端负责确认、提交与观察持久化结果。
CLI 等待进程退出不会中止已接受的操作。daemon 按持久化事务恢复中断现场；修复
及 switch 事务提交后补写操作结果，不能因前端未收到结果而重新执行。

### 8. 手工备份与恢复

`lkit backup create` 是不改变运行态的只读管理操作：取得安装锁后拒绝未完成事务，
不驱动恢复，即使 daemon 不可用也不修改现场。安装可用时读取当前 state、校验运行实例
和路径，通过配置导出 API 取得 TOML，再按 `.lkb` v1 minimal 规则流式生成并自校验最终
文件。它不创建业务事务、不停止服务、不更新 `install-state.json`。导出目标先检查；
后续外部复制失败时保留已验证的本地 `.lkb`，错误明确给出备份 ID 和路径。

`lkit restore` 使用独立的 schema v14 `restore` 事务，所有新恢复都要求网络确认：

1. 前端确认目标、备份范围和当前安装身份，daemon 重新验证目标 `.lkb`、架构和归档；
2. 默认创建当前实例的保护 `.lkb`，记录目标备份、保护备份和独立网络确认依据；
3. 保存 `previous_current`、原 state、服务事实、必要的 DNS 文件及原 data 的设备号和 inode；
4. 在 `prepared` 后进入 `stopping`，将旧 `data/` 原子移动到安装文件系统的事务工作目录；
5. 在 `activating` 从目标备份重建版本目录、空 data、初始化配置和 Geo 缓存；
6. 进入 `verifying`，通过启动和稳定健康检查后保存 pending state，开始网络确认窗口。
   旧已提交 state 保持不变；目标 state 的 `static_archive` 身份从备份内压缩包计算，
   仓库来源记录（`config.toml`）保持不变；
7. 用户用备份中的网络重新连接并验证访问后执行 `lkit network confirm`。daemon 复核
   安装身份、后端、PID、健康、期限与主机启动标识，持久化 `finalizing` 后写入目标
   state 并提交 `committed`；旧 data 事务现场与备份保留。

开始恢复时的 `--yes` 不替代应用后的网络确认。待确认期间 daemon 重启不延长期限；
期限到达、确认前整机重启或手工回滚均恢复原现场。`finalizing` 表示确认决定已经
接受，之后 daemon 或主机重启继续提交，不能因期限到达而改为回滚。

目标激活或健康检查失败时进入 `rolling_back`，优先使用身份匹配的原 data、release
和 state，必要时使用保护 `.lkb` 做配置级重建。每次回滚重试撤销旧启动放行凭据；原
配置恢复后写入当前主机启动标识，才允许启动原服务。恢复成功进入 `rolled_back` 并
返回 `5`；恢复失败保留未完成阶段与依据，返回 `6` 并由 daemon 重试。
`--allow-no-backup` 只允许保护备份失败时继续，不跳过目标校验、用户确认、网络确认或
中断恢复事实。原 data 丢失且无可用保护备份时阻断恢复，不能将目标 data 误认作旧数据。

### 9. 卸载

`lkit uninstall` 是独立的 `uninstall` 事务,只处理已提交安装,不负责空目录或状态损坏
现场。它只卸载 landscape 安装根;lkit 地盘与常驻 daemon 不属于卸载范围。完整命令
规格见 [`lkit uninstall`](../commands/uninstall.md)。

1. 获取安装锁并恢复未完成事务;读取已提交 state(记录 landscape 根),不存在则参数错误;
2. 交互确认卸载计划与数据损失范围(非交互模式以 `--yes` 代替);网络接管特征警告
   (宿主网络服务已屏蔽并停止)只提示不阻断;
3. 默认创建保护 `.lkb`(`uninstall 前自动保护备份`,auto 标记为 true),失败阻断;
   `--allow-no-backup` 显式跳过;
4. 创建 `preparing` 卸载事务,记录 `systemd_before`、`previous_current`、备份引用和
   systemd 环境必要的 `/etc/resolv.conf` 备份现场;
5. 更新为 `stopping` 后停止服务并确认进程退出,再更新为 `activating`,
   执行 `disable`、注销注册链接和 `daemon-reload`;
6. 删除 landscape 根受管内容(`releases/`、`data/`、`service/` 与 `current`);
   `--keep-data` 保留 `data/`;lkit 地盘(`config.toml`、`backups/`、`transactions/`)
   原样保留;
7. 提交 `committed`,输出卸载结果、保护备份 ID 与保留物清单。

卸载中断恢复采用前向完成:继续注销服务、删除文件并提交,不自动回滚;恢复再次失败
保留未完成阶段与现场，由 daemon 重试并阻止后续修改。卸载成功后 lkit 地盘视为无已提交安装,
再次 `lkit install` 按全新首次安装处理。

### 10. 重新初始化

`lkit reinit` 是独立的 `reinit` 事务，只接受已提交、由 systemd／OpenRC／SysVinit
管理且宿主网络服务已接管的安装。配置级重建不复制数据库字节文件，版本与 release
保持不变。完整规格见 [`lkit reinit`](../commands/reinit.md)。

1. 前端收集凭据与网络计划，展示破坏性计划并确认；确认绑定安装根、版本与状态摘要。
   拒绝时不创建事务或修改现场，密码通过独立受限输入文件交接。
2. daemon 领取请求并获取安装锁，复核安装身份、服务所有权与网络接管状态。
3. 创建并关联事务，保存原 state、服务状态、宿主网络与 DNS 恢复依据；默认创建保护
   `.lkb`，`--allow-no-backup` 显式跳过。任何恢复依据缺失均不能继续激活。
4. 停止服务后将旧 `data/` 移入事务现场，创建新 data 并写入新初始化配置，按计划清理
   新选 LAN 地址；桥接的创建、成员同步与清理由 Landscape 完成。
5. 启动服务并完成健康检查，写入 pending state，进入 `awaiting_network_confirmation`。
   操作记录为 `awaiting_confirmation`，释放执行线程与安装锁；普通修改请求等待。
6. 显式网络确认复核接口、管理地址、桥成员、PID 与健康后提交；确认期限到达、主机
   重启或手工回滚时，由 daemon 恢复旧 data、原服务与宿主网络状态。

崩溃恢复不重放凭据或工作流。激活前按事务恢复准备现场；激活后恢复原数据与服务；
已提交状态的身份匹配时补齐成功回执。回滚成功返回 `5`，未恢复完成保留未完成阶段并
返回 `6`，daemon 继续重试。待确认事务不会由 daemon 代替用户确认。

### 11. 手工部署迁移

`lkit migrate` 把运行中的手工部署迁移为同版本受管安装，使用独立 `migrate` 事务。
旧 unit 识别与接管要求 systemd，目标安装根必须全新，lkit 地盘不能已有已提交安装。
完整规格见 [`lkit migrate`](../commands/migrate.md)。

1. 前端只读检查源目录、进程身份与导出版本，展示计划并确认。请求绑定源目录身份、
   PID 与启动时刻、二进制与命令行摘要、旧 unit 与版本；拒绝确认不创建事务或备份。
2. daemon 在安装锁下重新验证，先关联操作与事务，再创建完整迁移 `.lkb`，包含导出
   配置、运行二进制和现场 static 内容；备份校验通过才进入停止阶段。
3. 修改旧 unit 前保存文件身份与 enabled/active 状态，再停止、禁用和解除注册冲突。
   无受管 unit 时提示 PID，等待用户从另一会话停止源进程；超时失败，不能以确认
   代替进程退出，不自动杀死无受管 unit 的源实例。
4. 从备份重建 release、data 与初始化配置，原子创建 current，注册、启用和启动
   新受管服务，完成健康检查后写入带本次事务 ID 的 state 并提交。

前端断开只停止观察；迁移领取后不允许取消。准备中断不重新执行备份，停止或激活
中断恢复原 unit 并清理未提交目标；同事务的已提交安装状态可补齐成功回执。回滚失败
保留 `rolling_back` 供 daemon 重试；源目录始终保留，无受管 unit 的前台实例由用户
自行重新启动。成功后保留备份，输出旧部署清理提示。

### 12. lkit 自身生命周期

lkit 自身(CLI 与全局常驻 daemon)与 landscape 安装解耦,由 [`lkit self`](../commands/self.md)
管理,不创建业务事务:

- `lkit self install`:把 lkit 注册为全局常驻服务(`/usr/local/lib/lkit/lkit.service`,
  `ExecStart=/usr/local/bin/lkit daemon`,注册链接指向全局原件),启用并启动,校验
  MainPID 非零;重复执行时 restart 刷新;
- `lkit self upgrade [--version <TAG>]`:从 GitHub Release 下载对应架构二进制与
  `SHA256SUMS`,校验、`--version` 自检后原子替换 `/usr/local/bin/lkit`;与目标版本
  相同时返回 `0` 不修改;daemon 已注册且运行则 `restart`,未运行不启动,未注册仅升
  CLI 并提示;
- `lkit self remove`:停止并注销 daemon,删除全局 unit 原件;幂等可重复,不删除
  `/usr/local/bin/lkit`,不触碰 lkit 地盘元数据。

daemon 固定读取 `/root/.lkit/`,pidfile 写入 `/root/.lkit/run/lkit.pid`,周期恢复
目标从 lkit 地盘状态与事务发现 landscape 根。卸载 landscape 不停止 daemon;daemon
在无未完成事务时保持空闲。

## 首次安装失败

首次安装启动失败时：
- 停止失败服务；
- 恢复或移除本次创建的 `current`；
- 按事务中的 `systemd_before` 恢复 systemd 注册链接以及 enabled/active 状态并 reload；
- 按事务中的 `resolv_conf_backup` 恢复 `/etc/resolv.conf` 原始状态；
- 不写入成功状态；
- 保留必要的 root-only 日志和事务信息；
- 将事务标记为 `failed`。

不得留下已启用但无法启动的 Landscape 服务。
