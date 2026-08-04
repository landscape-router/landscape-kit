# `lkit install`

仅用于首次安装 Landscape Router。目标目录中已经存在有效安装状态时返回参数使用错误，并提示使用 `switch`、`repair`、`reconcile` 或 `service-manager`。

```text
lkit install [--version <VERSION>] [--repository [<BASE_URL>]]
             [--install-dir <PATH>] [--admin-user <NAME>]
             [--password-file <PATH>] [--service-manager <systemd|none>]
             [--force] [--takeover-network]
```

- 未指定版本时安装最新 stable 版本；`--version latest` 与其等价。
- `--repository` 使用默认 HTTP 镜像；带值时使用指定 protocol v1 HTTP 仓库；缺省使用官方 GitHub provider。
- `--service-manager` 只表示首次安装的运行管理模式；缺省时自动选择。
- `--force` 不删除文件，只显示规范化安装根目录并要求用户自行清理。
- `--takeover-network` 仅用于首次安装，要求 systemd 和交互终端。它让用户选择 WAN/LAN
  接口，并在 Landscape 健康后进入待确认状态；完整行为见[网络接管](../network/takeover.md)。
- 已安装环境的版本切换、修复和状态变更不再通过本命令的互斥 flags 表达。

安装不按发行版名称设置白名单；使用 glibc 的 Linux 主机在通过完整运行能力预检后可以
继续。依赖缺失错误会按检测到的 `apt`、`dnf`/`yum`、`pacman` 或 `zypper` 显示安装建议。

交互式安装建议先单独安装 `lkit`，再从终端运行 `sudo lkit install ...`，确保进程可以
通过 `/dev/tty` 隐藏读取密码。无 TTY 的自动化环境必须使用 root 所有且权限为 `0400` 或
`0600` 的 `--password-file`。
