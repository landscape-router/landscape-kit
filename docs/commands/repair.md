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
- 官方路径的前端源不可达时：交互环境询问是否回退官方页面，非交互环境报错并提示
  使用 `--official`。

自定义前端包的格式、发布协议与校验规则见[前端开发规范](../frontend/developer.md)。

## 服务定义修复

`lkit repair service` 将现有 Landscape 的受管服务定义更新为当前 lkit 对应后端的
定义，包含 `lkit network startup-check`。支持 systemd、OpenRC 和 SysVinit；不切换
后端、不下载资产、不修改 Landscape 配置和数据、不创建 `.lkb`，不接受仓库参数。
`repair static` 和 `repair binary` 不承担服务定义迁移。

命令要求已提交的安装、可用的匹配后端、支持服务定义修复事务的运行中 daemon 和有效安装锁。存在未完成事务时拒绝迁移。
写入前验证安装状态中的定义摘要及系统注册链接；定义被手工修改、注册指向外部路径
或身份无法验证时拒绝覆盖，并报告冲突路径。

事务保存原定义字节、权限、注册信息、开机启动状态和安装状态快照。新定义先在临时
文件中生成并校验，再原子替换；systemd 执行 daemon-reload，OpenRC／SysVinit 使用
各自的定义校验和注册检查。验证后提交新定义摘要。更新不启动、停止或重启 Landscape，
不改变启用状态、runlevel 或 SysVinit 启停链接顺序。重复修复已匹配的定义不改变现场。

进入提交阶段前失败或进程中断，恢复原定义、权限、注册和安装状态；必要时重新加载服务管理器。
提交阶段持久化后，中断恢复续做新定义校验与安装状态提交。
恢复失败保留事务和备份，由恢复入口重试，不能标记修复成功。提交成功后可以执行 reinit。
