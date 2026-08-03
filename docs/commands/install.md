# `lkit install`

仅用于首次安装 Landscape Router。目标目录中已经存在有效安装状态时返回参数使用错误，并提示使用 `switch`、`repair`、`reconcile` 或 `service-manager`。

```text
lkit install [--version <VERSION>] [--repository [<BASE_URL>]]
             [--install-dir <PATH>] [--admin-user <NAME>]
             [--password-file <PATH>] [--service-manager <systemd|none>]
             [--force]
```

- 未指定版本时安装最新 stable 版本；`--version latest` 与其等价。
- `--repository` 使用默认 HTTP 镜像；带值时使用指定 protocol v1 HTTP 仓库；缺省使用官方 GitHub provider。
- `--service-manager` 只表示首次安装的运行管理模式；缺省时自动选择。
- `--force` 不删除文件，只显示规范化安装根目录并要求用户自行清理。
- 已安装环境的版本切换、修复和状态变更不再通过本命令的互斥 flags 表达。
