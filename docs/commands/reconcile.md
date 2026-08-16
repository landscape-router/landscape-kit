# `lkit reconcile`

检查活动版本，并协调受管元数据的外部变化。

```text
lkit reconcile [--repository [<BASE_URL>]] [--accept-service-change]
```

该命令不切换版本、不修复发布资产。它用于确认受管 systemd unit 内容变化、核对显式
仓库覆盖的资产身份（见[配置文件](../deployment/config.md)），以及观察
pending 初始化是否已经完成。普通 reconcile 不读取配置，显式 `--repository` 完全绕过
配置；两种情况下都不会写入或修改 `config.toml`。landscape 根从 `install-state.json`
发现，命令不接收 `--install-dir`。初始化完成后，现场
`data/landscape_init.toml` 的内容或存在性不属于受管元数据，reconcile 不读取也不改写它。
