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
- 首次安装未指定 `--service-manager` 时按 systemd 可用性自动选择，显式指定时严格使用目标模式。
- systemd 模式下服务链接、启用、启动和健康检查成功。
- 可能改变 systemd/Landscape 运行态的生产命令由临时 systemd unit 托管；杀掉等待
  结果的 SSH/CLI 前端后，worker 仍能提交或回滚并清理临时 unit。
- 无 systemd 时不启动、不停止、不检查健康；文件激活成功后记录 `verified: false` 并输出参考启动命令。
- 已安装环境未指定 manager 时保持当前模式，不发生隐式迁移。
- `systemd → none` 迁移停止并注销受管服务、提交 `manager: none` 并保持 Landscape 停止；失败时按事务恢复原 enabled/active 状态。
- `none → systemd` 迁移要求 `/dev/tty` 确认外部实例已停止并确认端口释放；启动验证成功后提交 `manager: systemd`，失败时撤销 systemd 接管但不尝试恢复未知的外部启动方式。
- `none → systemd` 可以接管 `initialization.status: pending` 的安装，并在完整初始化验证成功后提交为 `complete`。
- service manager 迁移不能与版本切换、仓库变化或 repair 合并执行。
- systemd 环境的运行态验证固定检查 UDP `53`、TCP `6300`、TCP `6443` 和
  `/api/docs`；无 systemd 环境不执行运行态验证。
- systemd 环境按本文执行 180 秒启动等待和 10 秒稳定观察；无 systemd 环境不执行这些检查。

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
- 数据库、API token、日志、metric 和 socket 不进入归档。
- 需要备份的路径只有在 `.lkb` 完整自校验成功后才允许停止当前服务。
- 有 `.lkb` 时，目标版本失败后用它创建空 data 并重新初始化旧版本。
- 无备份切换失败时仍恢复 `current`、服务状态和 `/etc/resolv.conf`，但不执行 data
  重建，也不宣称恢复目标版本可能修改的数据。
- 回滚成功恢复核心配置和 Geo 数据；不宣称恢复被明确排除的数据。
- `lkit restore` 只接受已有有效安装和完整验证的 `.lkb`，目标版本可与当前版本相同、较低
  或较高，不经过仓库下载。
- restore 默认在停止服务前创建当前实例保护 `.lkb`；`--allow-no-backup` 才能跳过该保护，
  且必须由用户确认承担风险。
- restore 的目标 `.lkb` 不包含 SQLite；恢复会创建空 data、重新初始化配置，并保留恢复前
  data 的事务现场，不得声称恢复数据库。
- systemd restore 必须恢复服务并通过完整健康检查后提交；none restore 必须保持停止并提交
  `initialization.status: pending`、`verified: false`。
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
