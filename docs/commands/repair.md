# `lkit repair`

修复现有安装的受管发布资产。

```text
lkit repair static [--repository [<BASE_URL>]] [--install-dir <PATH>]
lkit repair binary [--repository [<BASE_URL>]] [--install-dir <PATH>]
```

- `static`：备份当前静态目录并恢复活动版本的官方静态页面，不创建 `.lkb`。
- `binary`：重新下载并校验活动版本后端；systemd 环境创建 `.lkb` 并执行完整健康检查，无 systemd 环境只替换已停止实例的二进制。
- 两种修复都只允许现有安装，并且不会隐式切换版本。
- 未指定 `--repository` 时按 显式 CLI > `config.toml` > 官方 GitHub 的优先级解析来源
  （文件缺失时官方 GitHub，损坏时报错阻断，见[配置文件](../deployment/config.md)）。
- repair 始终验证本次实际使用的资产：static repair 对比 state 中的 static archive 身份，
  binary repair 对比解压后的后端身份；不一致则拒绝。成功后不写入、不修改 `config.toml`。
