# 验收标准

### 安装状态与路径

- 默认安装到 `/root/.lkit/landscape`，CLI 和环境变量优先级正确。
- 根路径软链接解析后仍能识别同一安装；内部受管目录逃逸时阻断。
- `install-state.json` 按固定 Schema 写入，损坏状态不被猜测重建。
- 同一 canonical 安装根目录不能并发运行两个事务；第二个进程立即因非阻塞锁冲突退出。
- 锁文件残留但没有进程持锁时不阻断安装。
- 状态、事务、服务定义和 `current` 使用本文规定的原子替换；失败时不留下部分 JSON 或缺失的 `current`。
- 非空未知目录不会被覆盖；`--force` 只提示手工清理。
- 已有 release 完全满足可信复用规则时复用；摘要不同、残缺、不可读或为符号链接时阻断且不修改原目录。

### 初始化与服务

- 首次安装必须设置密码，且密码不出现在命令行、日志、状态或事务中。
- 密码和确认不读取 stdin；无 `/dev/tty` 时要求对应的非交互专用参数。
- 密码文件接受 root 所有且 group/other 无权限的 `0400` 或 `0600` 普通文件，拒绝符号链接、非 root 所有者、多行、NUL 和超过 `4 KiB` 的内容。
- 初始化成功后保留 `landscape_init.toml`，权限为 `0600`，并生成初始化锁和持久配置。
- pending 初始化要求 `landscape_init.toml` 是 root 所有、权限 `0600` 的普通文件；
  complete 后不跟踪其内容或存在性，初始化锁缺失仍不可绕过。
- 首次安装要求 systemd 可用；服务链接、启用、启动和健康检查成功。
- 可能改变 systemd/Landscape 运行态的生产命令由临时 systemd unit 托管；杀掉等待
  结果的 SSH/CLI 前端后，worker 仍能提交或回滚并清理临时 unit。
- systemd 不可用时安装明确失败（退出码 `2`），不创建事务、不写文件；
  不支持无 systemd 的部署。
- 运行态验证固定检查 UDP `53`、TCP `6300`、TCP `6443` 和 `/api/docs`。
- 按本文执行 180 秒启动等待和 10 秒稳定观察。

### 备份与回滚

- `lkit backup create` 在不停止服务的情况下通过导出 API 创建 `auto: false` 的 `.lkb`，
  支持 remark 和可选外部输出路径；`list`、`show`、`verify` 不修改安装现场。
- `backup list` 不跟随符号链接；损坏、路径不安全或权限过宽的备份不会被当作有效备份。
- `backup verify` 完整检查 header、metadata、零填充、tar.gz checksum、路径逃逸和条目类型；
  verify 失败不创建或修改安装状态。
- 运行中的 systemd 服务在版本切换前必须成功调用导出 API，不能退回旧初始化文件；
  `--allow-no-backup` 不得绕过该要求。
- systemd 服务已停止时，版本切换默认拒绝；仅显式 `--allow-no-backup` 时允许跳过导出
  和 `.lkb`，并在事务中记录 `no_backup: true`。后端 repair 不允许该例外。
- `.lkb` Header、Metadata、零填充、tar.gz 和 checksum 均按 v1 校验；归档中的
  `landscape_init.toml` 必须保留并参与完整性校验。
- minimal 归档包含当前二进制、当前静态页面、API 导出配置和 `geo_tmp`。
- 数据库、API token、日志、metric 和 socket 不进入归档（数据库记录在恢复时按备份的
  `landscape_init.toml` 重建）。
- 需要备份的路径只有在 `.lkb` 完整自校验成功后才允许停止当前服务。
- 有 `.lkb` 时，目标版本失败后用它创建空 data 并重新初始化旧版本。
- 无备份切换失败时仍恢复 `current`、服务状态和 `/etc/resolv.conf`，但不执行 data
  重建，也不宣称恢复目标版本可能修改的数据。
- 回滚成功恢复核心配置和 Geo 数据；不宣称恢复被明确排除的数据。
- `lkit restore` 只接受已有有效安装和完整验证的 `.lkb`，目标版本可与当前版本相同、较低
  或较高，不经过仓库下载。
- restore 默认在停止服务前创建当前实例保护 `.lkb`；`--allow-no-backup` 才能跳过该保护，
  且必须由用户确认承担风险。
- restore 的目标 `.lkb` 不包含 SQLite 数据文件；恢复会创建空 data、重新初始化配置并
  保留恢复前 data 的事务现场。数据库以备份的 `landscape_init.toml` 重建（版本锁定），
  不得声称字节级数据库恢复。
- restore 必须恢复服务并通过完整健康检查后提交。
- restore 目标失败但原状态恢复成功返回 `5`；恢复失败返回 `6`，保留目标 release、旧
  data、保护备份和事务日志。
- 目标版本或后端 repair 失败但回滚成功时返回 `5`；回滚失败或需要人工恢复时返回 `6`。
- systemd 注册、enabled、active 和 `/etc/resolv.conf` 按事务记录恢复，缺少必要事实时不猜测。

### 修复与冲突

- 后端摘要变化必须确认或使用 `lkit repair binary`，不会被静默信任或覆盖。
- 普通安装不检查静态页面；`lkit repair static` 才恢复发布页面。
- 显式 `--repository` 覆盖无需二次确认；同版本仓库覆盖的 static 和后端资产身份必须
  与当前安装完全一致；`lkit` 从不写入 `config.toml`，来源解析按 显式 CLI > 配置 > 官方
  GitHub 的优先级进行，`config.toml` 存在但损坏时只有需要仓库的命令报错阻断，不静默回落。
- 截断进程名不被用作确认依据；识别使用 `/proc/<pid>/exe`、摘要、参数和端口关联。
- 未知进程或 systemd unit 所有权冲突阻断安装。
- 中断事务按阶段恢复，不无限重试，也不误报成功。
- stop 前必须先持久化 `stopping`；v1 `prepared` 和 v2 `prepared/stopping` 恢复均会
  幂等恢复事务前 enabled/active 状态。
- 参数错误返回 `2`，普通安全失败返回 `1`，并发锁冲突使用 `1` 并输出明确错误。
- 密码、API token、Authorization header 和带 query/fragment 的 URL 不进入终端输出或事务日志。
- 非 Debian 的 glibc Linux 发行版不因 `/etc/os-release` 的 `ID` 被拒绝；仍必须通过完整
  的内核、BPF、Cgroup、依赖、端口和服务检查。依赖错误保留可执行的包管理器安装建议。

### 重新初始化

- `lkit reinit` 只接受已提交、`service.manager == systemd` 且宿主网络服务已被接管的
  安装;目标目录无有效状态、非 systemd 或未接管时返回参数错误,不隐式接管。
- 凭据与网络计划在交互中收集,破坏性计划确认先于任何修改;确认被拒或非交互缺少
  `--yes` 时不创建事务、不写任何文件、不停止服务。
- reinit 默认在停止服务前创建保护 `.lkb`(备注 `reinit 前自动备份`、auto 为 true),
  失败阻断;`--allow-no-backup` 显式跳过并记录 `no_backup: true`。
- 新 `landscape_init.toml` 的 `version` 固定为当前活动版本,只包含新凭据与用户选择的
  WAN/LAN 网络实体;其余配置实体全部清空并由 Landscape 重建数据库,release 与静态
  资产逐字节不变,`config.toml` 不读取不修改。
- install 与 reinit 都不检查 `br_lan` 是否存在;桥接的创建、成员同步与清理由 Landscape
  按新配置处理。新选 LAN 接口执行地址 flush,WAN 不清理。
- 健康检查通过后一律进入 `awaiting_network_confirmation`,arm 恢复二进制、10 分钟
  timer 与 boot rollback,不直接提交;`lkit network confirm` 复核接口 MAC、管理地址、
  `br_lan` 成员、PID 与健康后提交。
- 确认前重启、timer 到期与手工 rollback 走同一幂等回滚:停止服务 → 恢复旧 `data/` →
  重启旧配置并通过健康检查;回滚成功 `rolled_back`,失败 `failed` 并保留现场与保护
  备份。
- 激活或健康检查失败但自动回滚成功返回 `5`;回滚失败返回 `6`。
- 中断事务按阶段恢复:`preparing` 标记 `failed`;`prepared`/`stopping` 恢复事务前
  systemd 状态;`activating`/`verifying` 执行旧 data 回滚;待确认阶段阻断并提示
  `lkit network confirm`/`rollback`。

### 手工部署迁移

- `lkit migrate --from` 只接受含 Landscape 特征文件（`landscape.toml` 或
  `landscape_init.lock`）的真实目录，拒绝受管安装的 data 目录；目标安装根必须全新
  （无 state、无遗留 data/releases/service/current）。
- 迁移要求旧实例运行中：按固定端口定位并用 `--config-dir` 参数确认实例身份，
  通过导出 API 读取当前配置与后端版本；端口上有无法确认身份的进程时阻断。
- 迁移备份 `.lkb` 记录旧版本（不升级），生成后保留在 `backups/`；`static.zip` 本地
  缺失时从发布仓库下载，仓库不可用时从 `static/` 现场打包并自校验。
- 确认先于停止：拒绝或非交互缺 `--yes` 时不创建事务、不写任何文件、不停旧实例。
- 旧 unit 按 `ExecStart --config-dir` 发现：唯一匹配才接管（stop/disable，原件位于
  `/etc/systemd/system` 时移入事务目录）；多匹配阻断；无匹配或进程仍存活时要求用户
  确认前台实例已停止。
- 注册、启用、启动新受管服务并通过完整健康检查后提交 `initialization.status: complete`。
- 停止旧实例后任何失败自动回滚：注销/停止新受管 unit、恢复 `/etc/resolv.conf`、
  恢复旧 unit 的 enabled/active 状态并重启、删除新根内容；回滚成功 `rolled_back`
  返回 `5`，失败 `failed` 返回 `6`。
- 迁移不删除、不修改旧部署目录与旧二进制；成功后旧 unit 保持停止并提示用户自行清理。
- 中断迁移按阶段恢复：`preparing` 标 `failed`（备份保留）；`prepared`/`stopping`
  恢复旧 unit；`activating`/`verifying` 执行与失败相同的回滚。

### 卸载

- `lkit uninstall` 只接受已有有效 `install-state.json` 的安装；无状态返回 `2`，损坏状态
  不被猜测重建。
- 卸载前默认创建保护 `.lkb`（备注 `uninstall 前自动保护备份`、auto 标记为 true），失败
  阻断；`--allow-no-backup` 显式跳过并记录 `no_backup: true`。
- 非交互模式必须提供 `--yes`，否则返回 `2` 且不创建事务、不写任何文件。
- 按 stop → disable → 注销注册链接 → `daemon-reload` 的顺序清理。
- 默认保留 `config.toml`、`backups/` 与 `transactions/`；`config.toml` 内容逐字节不变。
- `--keep-data` 保留 `data/` 并删除其余受管内容；`--purge-root` 整树删除安装根目录且
  必须同时给出 `--allow-no-backup`；两者与缺参组合都返回 `2`。
- 网络接管特征（宿主网络服务被 stop/disable/mask）在卸载前输出警告但不阻断，卸载不
  恢复宿主网络服务。
- 卸载中断恢复采用前向完成，不自动回滚；恢复再次失败标记 `failed` 并保留保护 `.lkb`
  与事务现场供人工诊断。
- 卸载成功后该根目录不存在 `install-state.json`，再次 `lkit install` 按全新首次安装处理。
- 卸载只定义退出码 `0/1/2` 和 `130`，不定义 `5/6`。
