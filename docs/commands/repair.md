# `lkit repair`

修复现有安装的受管发布资产。

```text
lkit repair static [--repository [<BASE_URL>]] [--official]
lkit repair binary [--repository [<BASE_URL>]]
```

- `static`：备份当前静态目录并恢复活动版本的静态页面，不创建 `.lkb`。恢复目标按
  配置的意图决定：激活了自定义前端源（[`[frontend]`](../deployment/config.md)）时
  重新拉取该前端源的 latest/stable 并应用；否则恢复官方页面。
- `--official`：无条件恢复官方静态页面（不解析自定义前端源），并在配置了自定义
  前端源时提示下次 switch/update 会重新应用自定义前端。
- `binary`：主线对照与转正入口。先获取活动版本主线后端的落盘形态哈希（GitHub
  `SHASUM256sum.txt`、HTTP manifest 的 `sha256_decompressed`，缺失时下载解压
  计算），与本地磁盘二进制比对：
  - **一致**（含可复现的自定义构建）→ 纯元数据转正：只更新 state 的后端身份
    （完整性锚点与主线对照身份一致化），不下载替换、不停止服务、不创建 `.lkb`；
  - **不一致** → 下载主线后端并校验解压替换；创建 `.lkb` 并执行完整健康检查，
    提交后状态回到 `official`。
- 两种修复都只允许现有安装，并且不会隐式切换版本。landscape 根从
  `install-state.json` 发现，命令不接收 `--install-dir`。
- 未指定 `--repository` 时按 显式 CLI > `config.toml` > 官方 GitHub 的优先级解析来源
  （文件缺失时官方 GitHub，损坏时报错阻断，见[配置文件](../deployment/config.md)）。
  前端源解析是**宽容**的：`config.toml` 损坏或缺失时 `repair static` 按官方修复
  处理，保证显式 `--repository` 在配置损坏时仍能绕过配置工作。
- repair 始终验证本次实际使用的资产：下载物与本次解析来源的元数据（manifest /
  `SHASUM256sum.txt`）严格一致；官方路径成功后更新 state 中的 static archive 身份
  并刷新版本目录 `static.zip`（恢复备份/手工替换造成的身份漂移）。binary repair
  的对照基准是主线声明的落盘形态哈希而不是 state 记录——对照一致时转正，不一致时
  下载替换，两者都不再因"与 state 记录不符"而拒绝；替换路径创建的保护 `.lkb`
  如实快照修复前的二进制并标注身份。活动版本不在主线存在时，对照物无法获得，
  repair 报错并提示出路（见
  [后端合法性与主线对照](../deployment/backend-legality.md#转正与逃生路径)）。
  成功后不写入、不修改 `config.toml`。
- 官方路径的前端源不可达时：交互环境询问是否回退官方页面，非交互环境报错并提示
  使用 `--official`。

自定义前端包的格式、发布协议与校验规则见[前端开发规范](../frontend/developer.md)。
