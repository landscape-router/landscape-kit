# 发布、安装与成功切换

## Release 定义

| 版本 | 场景 | 用途 |
| --- | --- | --- |
| `1.0.0` | `healthy` | 首次安装基线 |
| `2.0.0` | `healthy` | 成功切换目标 |

每个 release 的版本同时用于：

- `lkit-fixture-release --version`；
- `lkit-fixture-release --stamp-version` 写入的二进制尾部标记；
- `static/lkit-fixture.json.export_version`；
- `lkit-publish --version`。

版本只有一个输入来源，不允许分别手写。

## 发布 `1.0.0`

1. 生成 native fixture binary、另一架构占位资产和 `static.zip`。
2. 使用真实 `lkit-publish` 上传到临时 RustFS bucket。
3. 验证 `repository.json`、`releases/1.0.0/manifest.json` 和 `channels/stable.json`。
4. 验证 stable pointer 为 `1.0.0`。

## 首次安装

runner 使用带 `test-support` 的 CLI，并由公共 wrapper 自动追加
`--test-runtime <runtime.json>`：

```sh
lkit install \
  --version 1.0.0 \
  --repository <rustfs-public-base> \
  --install-dir /var/lib/lkit-e2e/landscape \
  --admin-user admin \
  --password-file /var/lib/lkit-e2e/password \
  --service-manager systemd
```

必须验证：

- `state/install-state.json.active_version` 为 `1.0.0`；
- `current` 指向 `releases/1.0.0`；
- state architecture 等于 runner 原生架构；
- unit 已注册、enabled 且 active；
- MainPID 非零；
- `/proc/<pid>/exe` SHA 等于 state 中记录的后端 SHA；
- TCP/UDP 53、TCP 6300、TCP 6443 通过健康检查；
- HTTPS `/api/docs` 返回成功；
- 初始化文件存在且 fixture API token 权限为 `0400`。

## 成功切换 `2.0.0`

1. 在运行中的 `landscape.toml` 写入唯一用户标记。
2. 调用导出 API，确认返回内容包含该标记。
3. 发布 `2.0.0`，确认 stable pointer 前进到 `2.0.0`。
4. 确认 `1.0.0` 和 `2.0.0` fixture binary SHA 不同。
5. 执行：

```sh
lkit switch \
  --version 2.0.0 \
  --install-dir /var/lib/lkit-e2e/landscape
```

必须验证：

- 命令退出码为 `0`；
- 最新事务 phase 为 `committed`；
- active version 和 `current` 都切换到 `2.0.0`；
- fake systemctl 报告的 MainPID 运行 `2.0.0` 后端 SHA；
- 用户配置标记没有被 fixture 重启覆盖；
- 自动 `.lkb` metadata 的 `landscape_version` 为 `1.0.0`；
- metadata 的 `auto=true`、`scope=minimal`、architecture、backup ID 和 tar checksum 均合法。
