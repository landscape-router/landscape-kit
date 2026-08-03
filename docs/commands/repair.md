# `lkit repair`

修复现有安装的受管发布资产。

```text
lkit repair static [--repository [<BASE_URL>]] [--install-dir <PATH>]
lkit repair binary [--repository [<BASE_URL>]] [--install-dir <PATH>]
```

- `static`：备份当前静态目录并恢复活动版本的官方静态页面，不创建 `.lkb`。
- `binary`：重新下载并校验活动版本后端；systemd 环境创建 `.lkb` 并执行完整健康检查，无 systemd 环境只替换已停止实例的二进制。
- 两种修复都只允许现有安装，并且不会隐式切换版本。
- 未指定 `--repository` 时沿用 state 记录的仓库；显式覆盖仓库无需再次确认，但新来源
  必须提供与当前版本完全相同的 static 和后端资产。
