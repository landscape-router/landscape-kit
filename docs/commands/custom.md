# `lkit custom`

从本地目录部署用户提供的后端二进制与静态页面。它面向两类场景：

- 开发者把自编译的 Landscape 构建受控地安装到机器上；
- 离线介质安装：把从官方渠道取得的资产拷贝到目标机器后本地安装，联网时经主线
  对照确认后等同官方安装。

`custom` 是**受控部署通道**，不是合法化通道：部署内容若与主线不一致，安装后状态
仍是 `custom`（非主线），升级门槛与转正路径见
[后端合法性与主线对照](../deployment/backend-legality.md)。

```text
lkit custom <DIR> [--repository [<BASE_URL>]] [--accept-drift]
```

- `--repository` 只用于主线对照（判定部署内容是否与主线一致），不用于下载部署
  内容；部署内容全部来自 `<DIR>`。未指定时按 显式 CLI > `config.toml` > 官方
  GitHub 的优先级解析对照来源（见[配置文件](../deployment/config.md)）。
- `--accept-drift` 见[已安装分支](#已安装分支)。
- `--non-interactive` 和 `--lang` 是全局参数。landscape 根从 `install-state.json`
  发现；未安装分支按 `lkit install` 的规则选择安装根（`--install-dir`、
  `LKIT_INSTALL_DIR` 或默认值）。

## 目录契约

`<DIR>` 必须是绝对或相对路径指向的真实目录（不跟随符号链接），固定包含：

```text
<DIR>/
├── landscape-webserver    # 后端二进制
├── static/                # 静态页面目录
│   └── index.html         # 至少包含此普通文件
└── release.toml           # 版本清单
```

- `landscape-webserver`：普通文件，非空，不得是符号链接；部署时统一设置 `0755`；
- `static/`：普通目录，至少含普通文件 `index.html`；安装时由 lkit 现场打包为
  `static.zip`（与 `.lkb` 备份相同的打包自校验），目录含符号链接、设备文件等非法
  条目时失败并指明条目；
- `release.toml`：TOML，必填 `version` 字段，取值必须是与当前主机架构兼容的
  规范化 stable SemVer（禁止 prerelease 与 build metadata）。

校验失败的目录在创建任何事务或修改任何文件之前拒绝（参数错误 `2`）。

## 未安装分支

机器上没有有效安装状态时，`custom` 走完整首次安装流水线：初始化配置、凭据输入、
网络接管选项、unit 注册、健康检查与 install.md 描述的约束全部与
[`lkit install`](install.md) 一致，唯一差别是部署资产来自 `<DIR>` 而不是仓库
下载。版本规则只要求 `release.toml` 的版本合法，没有"必须更高"的限制。

## 已安装分支

已有有效安装时，`custom` 走升级路径，复用 switch 流水线（事务、保护 `.lkb`、
systemd 托管、健康检查与自动回滚），并满足：

- **版本必须更高**：`release.toml` 的版本按 SemVer 高于当前活动版本；更低是参数
  错误，相同版本走既有的同版本安装校验而不是 custom；
- **完整性门槛**：磁盘二进制与 state 锚点一致时正常继续；完整性漂移（`drifted`）
  时默认拒绝，交互模式通过后端替换确认、非交互模式携带 `--accept-drift` 才能继续
  ——部署会把锚点重新绑定到 `<DIR>` 提供的二进制，这是漂移状态的受控恢复手段；
- **保护备份**：停止服务前照常创建 `.lkb`，如实快照当前内容（含漂移或非主线
  二进制）并在 metadata 标注，失败自动回滚与 switch 相同。

## 合法性判定

部署事务提交前，lkit 对 `<DIR>` 的二进制做尽力主线对照：

| 对照结果 | 提交的 `official_sha256` | 安装后状态 |
| --- | --- | --- |
| 与主线落盘形态哈希一致 | 主线哈希 | `official`（等同官方安装） |
| 主线存在该版本但不一致 | 主线哈希 | `custom` |
| 网络不可达、来源解析失败或版本不在主线 | `null` | `custom`（对照身份未知） |

对照失败不阻断部署。对照身份为 `null` 的安装事后可由 `lkit repair binary` 补做
对照：构建与主线逐字节一致（可复现构建）时直接转正，不一致时由 repair 下载主线
产物替换。

## 事务与退出码

- 两个分支都创建标准事务（`operation` 为 `custom`），提交时 `sha256` 锚定到
  `<DIR>` 部署的二进制，`official_sha256` 按上表记录；
- 退出码沿用升级族契约：成功 `0`，参数错误 `2`，普通失败 `1`，激活失败自动回滚
  成功 `5`，回滚失败 `6`，显式 Ctrl+C `130`（见
  [输出与退出码](output-and-exit-codes.md)）；
- 生产 systemd 环境中的委托、控制台分发与 worker 语义与 switch 一致。
