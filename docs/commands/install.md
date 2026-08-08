# `lkit install`

仅用于首次安装 Landscape Router。目标目录中已经存在有效安装状态时返回参数使用错误，并提示使用 `update`、`switch`、`repair`、`reconcile` 或 `service-manager`。

```text
lkit [--non-interactive] install [--version <VERSION>]
             [--repository [<BASE_URL>]]
             [--install-dir <PATH>] [--admin-user <NAME>]
             [--password-file <PATH>] [--service-manager <systemd|none>]
             [--force] [--takeover-network]
```

- 未指定版本时安装最新 stable 版本；`--version latest` 与其等价。
- `--non-interactive` 是全局参数，可放在子命令前或后；它显式禁止打开终端、输入提示和
  动态终端进度。缺少必须的密码文件或确认参数时直接失败。
- `--repository` 使用默认 HTTP 镜像；`--repository github` 使用官方 GitHub 仓库；带值时
  使用指定 protocol v1 HTTP 仓库；缺省时按 显式 CLI > `config.toml` > 官方 GitHub 的
  优先级解析来源（预置配置生效，缺失时官方 GitHub）。
- 仓库来源不写入 `state/install-state.json`，`lkit` 也**从不创建或更新** `config.toml`；
  该文件完全由用户维护，只影响后续命令未显式指定 `--repository` 时的缺省来源，
  见[配置文件](../deployment/config.md)。
- `--service-manager` 只表示首次安装的运行管理模式；缺省时自动选择。
- `--force` 不删除文件，只显示规范化安装根目录并要求用户自行清理。
- `--takeover-network` 仅用于首次安装，要求 systemd 和交互终端。它让用户选择 WAN/LAN
  接口，并在 Landscape 健康后进入待确认状态；完整行为见[网络接管](../network/takeover.md)。
  管理控制台的 Install 面板始终启用该模式（开关暂隐藏，见代码中的
  `TODO(network-takeover)`）；CLI 的 `--takeover-network` 仍为显式参数。
- `--takeover-network` 只允许目标 `data/` 目录不存在或为空；已有数据必须先由对应的
  `lkit network rollback` 清理。确认前重启会触发 boot rollback，不会让本次安装继续等待。
- 已安装环境的版本更新、切换、修复和状态变更不再通过本命令的互斥 flags 表达。

如果安装根目录存在未完成的网络接管事务，`install` 不自行确认、回滚或删除数据，只提示
使用 `lkit network status`、`lkit network confirm` 或 `lkit network rollback`。`--admin-user`
和 `--password-file` 仍只允许真正的首次安装；回滚成功且未提交数据已清理后，才可再次提供
这些参数。

安装不按发行版名称设置白名单；使用 glibc 的 Linux 主机在通过完整运行能力预检后可以
继续。依赖缺失错误会按检测到的 `apt`、`dnf`/`yum`、`pacman` 或 `zypper` 显示安装建议。

人工安装建议先单独安装 `lkit`，再从终端运行 `sudo lkit`，从管理控制台侧栏进入 Install
面板。`lkit install` 保留为命令模式；没有 `--password-file` 时仍通过 `/dev/tty` 隐藏读取
密码。无 TTY 的自动化环境必须显式使用 `--non-interactive`，并提供 root 所有且权限为
`0400` 或 `0600` 的 `--password-file`。

交互终端中，部署前 warning 在密码提示前分行显示，隐藏输入的两次提示各自占一行。
下载后端和静态资源时分别显示包含已下载字节数、总字节数、百分比、速率和 ETA 的进度条；
stderr 不是终端时不输出动态进度，避免污染脚本日志。

命令模式和控制台的职责边界见 [Ratatui 管理控制台](../interaction/console.md)。交互执行
期间按 Ctrl+C 会恢复安装开始前的终端属性和可见光标。inline 操作立即以状态 `130` 退出；
systemd 托管操作先停止对应临时 worker，再以 `130` 退出。SSH/SIGHUP 断线不是显式取消，
worker 仍按事务规则继续完成提交或回滚。
