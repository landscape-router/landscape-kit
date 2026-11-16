# `lkit repair`

修复现有安装的受管发布资产或服务定义。

```text
lkit repair static [--repository [<BASE_URL>]] [--official]
lkit repair binary [--repository [<BASE_URL>]]
lkit repair service
```

- `static`：备份当前静态目录并恢复活动版本的静态页面，不创建 `.lkb`。恢复目标按
  配置的意图决定：激活了自定义前端源（[`[frontend]`](../deployment/config.md)）时
  重新拉取该前端源的 latest/stable 并应用；否则恢复官方页面。
- `--official`：无条件恢复官方静态页面（不解析自定义前端源），并在配置了自定义
  前端源时提示下次 switch/update 会重新应用自定义前端。
- `binary`：重新下载并校验活动版本后端；创建 `.lkb` 并执行完整健康检查。
- 所有修复都只允许现有安装，并且不会隐式切换版本。landscape 根从
  `install-state.json` 发现，命令不接收 `--install-dir`。
- 未指定 `--repository` 时按 显式 CLI > `config.toml` > 官方 GitHub 的优先级解析来源
  （文件缺失时官方 GitHub，损坏时报错阻断，见[配置文件](../deployment/config.md)）。
  前端源解析是**宽容**的：`config.toml` 损坏或缺失时 `repair static` 按官方修复
  处理，保证显式 `--repository` 在配置损坏时仍能绕过配置工作。
- static/binary repair 始终验证本次实际使用的资产：下载物与本次解析来源的元数据（manifest /
  `SHASUM256sum.txt`）严格一致；官方路径成功后更新 state 中的 static archive 身份
  并刷新版本目录 `static.zip`（恢复备份/手工替换造成的身份漂移）。binary repair
  对比解压后的后端身份；不一致则拒绝。成功后不写入、不修改 `config.toml`。
- 自定义前端源不可达时，本次操作失败并提示使用 `--official`。交互前端可在用户
  确认后提交新的官方修复请求；daemon 不读取终端，也不隐式回退。

自定义前端包的格式、发布协议与校验规则见[前端开发规范](../frontend/developer.md)。

## 执行、查询与取消

三种修复都由 daemon 直接执行，CLI／TUI 只提交结构化参数并展示持久化状态，不创建
lkit worker。daemon 必须支持对应操作类型；缺失或版本不匹配时拒绝提交，不能改为
前端内联执行。安装锁被占用或存在阻断事务时保持排队，领取后重新验证现场。
static／binary 请求绑定提交时的活动版本；排队期间版本变化会使请求失败。

提交后输出操作 ID。`lkit operation status <id>` 查询当前阶段、下载进度、可取消
状态及结果，`lkit operation list` 列举记录；前端断开后记录仍保留，重新查询不重放
操作。CLI 的 Ctrl+C 结束等待，不取消 daemon 工作流。

`lkit operation cancel <id>` 可以取消排队请求；static／binary 在下载阶段还允许
协作取消，完成临时文件清理后才报告取消成功。开始准备、备份、停服务或修改文件后
拒绝取消；service 领取后不允许取消。取消与阶段切换的竞争由 daemon 裁定，前端
展示的进度不构成修改许可。完整规则见[daemon 操作管理](../service/daemon.md)。

## 资产修复与恢复

static 修复不停止服务。修改前保存静态目录、原 `static.zip` 及旧安装状态；恢复
必须让这些对象保持一致，不能只恢复页面而保留新归档或新摘要。不创建 `.lkb`，不
重建 Landscape data。binary 修复按安装记录的 systemd、OpenRC 或 SysVinit 后端
管理服务；无法管理受管实例或无法完成导出与保护备份时，在修改前失败，不等待人工
停进程。激活失败使用 `.lkb`、旧二进制、旧状态和服务快照恢复，并执行相应健康验证。

两种资产修复提交前中断按各自事务恢复；事务已经提交但操作结果未写入时补写成功，
不重新执行。恢复失败保留诊断现场和阻断依据。详见
[Repair 阶段转换](../workflows/lifecycle.md#4-repair-阶段转换)。

## 服务定义修复

`lkit repair service` 将现有 Landscape 的受管服务定义更新为当前 lkit 对应后端的
定义，包含 `lkit network startup-check`。支持 systemd、OpenRC 和 SysVinit；不切换
后端、不下载资产、不修改 Landscape 配置和数据、不创建 `.lkb`，不接受仓库参数。
`repair static` 和 `repair binary` 不承担服务定义迁移。

命令要求已提交的安装、可用的匹配后端和支持服务修复操作的运行中 daemon。
写入前验证安装状态中的定义摘要及系统注册链接；定义被手工修改、注册指向外部路径
或身份无法验证时拒绝覆盖，并报告冲突路径。

事务保存原定义字节、权限、注册信息、开机启动状态和安装状态快照。新定义先在临时
文件中生成并校验，再原子替换；systemd 执行 daemon-reload，OpenRC／SysVinit 使用
各自的定义校验和注册检查。验证后提交新定义摘要。更新不启动、停止或重启 Landscape，
不改变启用状态、runlevel 或 SysVinit 启停链接顺序。重复修复已匹配的定义不改变现场。

进入提交阶段前失败或进程中断，恢复原定义、权限、注册和安装状态；必要时重新加载服务管理器。
提交阶段持久化后，中断恢复续做新定义校验与安装状态提交。
恢复失败保留事务和备份，由恢复入口重试，不能标记修复成功。提交成功后可以执行 reinit。
