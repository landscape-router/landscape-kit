# 配置文件（`config.toml`）

## 职责

`/root/.lkit/config.toml` 是 lkit 地盘顶层的**用户维护**配置文件，独立于
`state/install-state.json`。它保存安装机器的持续分发通道偏好，不属于安装记录：
`state/install-state.json` 不包含任何仓库信息。

`lkit` 不会凭空创建 `config.toml`；只有显式的偏好/通道配置动作才会写回该文件：

- 交互控制台按 `L` 切换语言会写回 `[ui] language`（见[语言预设](#语言预设)）；
- `lkit self install --flare-psk-file` 供给急救恢复码时写回 `[flare]` 段
  （见[flare 恢复通道](#flare-恢复通道)），daemon 首启无 `[flare]` 时也会生成
  随机 psk 并写回（恒常托管）；
- `lkit flare setup` 显式写回 `[flare]` 段。

仓库来源选择不会被任何命令持久化，成功、失败、回滚或中断恢复都不会改变该文件；
用户随时可以编辑或删除 `config.toml` 改变后续命令的缺省来源；文件不存在与
"官方 GitHub 默认"等价，首次安装不传 `--repository` 时行为一致。

`config.toml` 允许存在于 lkit 地盘顶层（`lkit` 对顶层目录执行白名单检查，
`releases/`、`state/` 等受管目录之外的未知文件都会阻断命令）。没有该文件时首次安装
正常进行。config.toml 不属于任何 landscape 安装根：卸载 landscape 不删除它。

## Schema v1

建议权限 `0600`（`lkit` 写回时强制，见 [flare 恢复通道](#flare-恢复通道)；用户手工
编辑时自行设置）。解析时未知字段和未知 section 允许并忽略。

```toml
schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[ui]
language = "zh"
```

HTTP 仓库来源示例：

```toml
schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape/"
```

字段规则：

- `schema_version` 固定为整数 `1`；
- `repository.kind` 只允许 `github` 或 `http`；
- `repository.location` 是仓库位置：GitHub 为 `owner/repo`，HTTP 为 protocol v1 base URL
  （读取时按与 CLI 相同的规则校验并规范化，例如补全尾部 `/`）；
- `repository.location` 不保存预签名 URL，不得包含凭据或敏感 query；
- `ui.language` 可选，只允许 `en` 或 `zh`；缺失、值不受支持（如 `fr`）或类型错误时
  忽略，语言解析回落到系统 locale（见[语言预设](#语言预设)）；
- 未知字段和未知 section 允许并忽略，供未来配置扩展。

## 来源解析优先级

所有命令的仓库来源统一按以下顺序解析：

1. **显式 CLI**：`--repository`（裸参数 = 默认 HTTP 镜像；`github` = 官方 GitHub 仓库；
   URL = 自定义 protocol v1 HTTP 仓库）。显式参数**完全绕过** `config.toml`，包括配置
   损坏的情况；
2. **配置**：`config.toml` 存在且有效时使用其中记录的来源；
3. **默认**：文件缺失时使用官方 GitHub provider（`ThisSeanZhang/landscape`）。

配置只在命令**实际需要仓库**时才读取：

- 需要读取：首次安装（配置驱动来源）、`install`/`switch`/`update`（解析版本）、
  `repair`（解析来源）；
- 不读取：普通 `reconcile` 同版本检查、`check`、`restore`、`backup`、
  `network` 子命令、`install --force`，以及不通过安装适用性
  检查就报错的命令（例如空目录上运行 `switch`/`reconcile` 或已有安装上再次
  `install`）。这些命令不受损坏配置影响。

文件**存在但损坏**（TOML 解析失败、`repository` section 或 `schema_version` 字段
缺失、字段类型错误、`schema_version` 不支持、`kind` 非法、HTTP URL 不安全或 GitHub
名称非法）或**不可读**（例如权限不足）时，读取它的命令报配置错误并阻断，提示修复
或删除 `config.toml` 以回落官方 GitHub 默认；损坏很可能来自用户编辑错误，静默回落
会让用户误以为配置仍然生效。

## 自定义前端（`[frontend]`）

`[frontend]` 段登记多个前端源并选择激活项。配置了自定义前端时，install/update/
switch 构建版本目录后会按激活源解析前端包并替换 `static/`（`static.zip` 官方基线
不变）；`repair static` 按激活源意图恢复。不配置该段或激活 `official` = 官方前端。
完整包格式与发布协议见[前端开发规范](../frontend/developer.md)。

```toml
[frontend]
active = "community"   # 激活源 id；缺省或 "official" = 官方前端

[[frontend.sources]]
id = "community"
name = "社区前端"        # 可选展示名
kind = "http"           # "http" | "github"
location = "https://frontend.example.com/ui/"

[[frontend.sources]]
id = "dark"
name = "暗色主题"
kind = "github"
location = "someone/dark-ui"
```

字段规则（严格校验，与 `[repository]` 同级）：

- `frontend.active` 可选；值只允许 `official` 或已登记 source 的 `id`，指向不存在
  的 id 时**阻断报错**并列出合法 id；
- `frontend.sources` 可选的数组；`id` 必填且唯一，`kind` 只允许 `github` 或
  `http`，`location` 按与 `[repository]` 相同的规则校验并规范化（GitHub 为
  `owner/repo`，HTTP 为 protocol v1 base URL）；
- `frontend.sources.name` 可选，仅展示；
- 段缺失、缺失 source 或 `active` 缺省时等价官方前端；
- 未知字段和未知 section 允许并忽略。

需要解析前端源的命令（install/update/switch/repair static）在段损坏时阻断并提示
修复或删除该段以回落官方；不需要前端源的命令不受影响。

## 语言预设

`[ui] language` 预设界面与命令输出语言（仅 `en` 或 `zh`）。语言在命令行解析完成后
按以下优先级解析：

1. 显式 CLI：`--lang`（放在子命令前后均可）；
2. 环境变量：`LKIT_LANG`；
3. 配置：`[ui] language`；
4. 系统 locale（`LC_ALL`/`LC_MESSAGES`/`LANG` 的主语言标签）；
5. 默认英文。

与仓库来源的严格校验不同，语言是**宽容读取**：`config.toml` 缺失、损坏、没有
`[ui] language`，或值不受支持时，该层被跳过、不阻断任何命令。损坏的配置文件仍然
会阻断需要读取仓库来源的命令（见上文），但不影响语言解析。

配置预设覆盖 CLI 输出、Ratatui 控制台、交互提示、进度与命令结果；clap 帮助与参数
错误在命令行解析阶段渲染，无法使用配置预设的语言，`--lang` 与 `LKIT_LANG` 可以。

交互控制台按 `L` 切换语言会原子写回 `[ui] language`，下次会话沿用；写回经
tmp + rename 完成，保留注释、未知 section/字段与原有顺序，并发读写安全，不会撕裂。
`config.toml` 缺失时切换会创建带默认仓库来源与语言的最小配置；TOML 损坏时切换
仍生效（只影响本次会话）但显示提示，且不改动原文件。CLI 命令只读配置预设，
`--lang` 与 `LKIT_LANG` 覆盖不写回文件。

## flare 恢复通道

`[flare]` 段配置 daemon 托管的 Landscape Terrain（L2 防失联通道）服务端。daemon 在
Linux 上**恒常托管** flare：段缺失或无 `psk` 时自动生成随机 psk 并写回本文件
（启动时打印一次分发提示），随后每个周期对比配置指纹，`[flare]` 变更（psk 非空）时
重启 flare 任务拾取新配置；psk 被清空则保持现役运行，不切断恢复通道。

```toml
[flare]
psk = "一个 ≥12 字符的共享密钥"
device_name = "landscape-router"   # 可选,默认 landscape-router
mac = "aa:bb:cc:dd:ee:ff"          # 可选,缺省自动探测
devices = "any"                    # 可选,默认 any
ethertype = 0x88b6                 # 可选,默认 0x88b6
forward_ports = "22,6443"          # 可选,默认 22,6443
token = "发现令牌"                  # 可选
```

写入路径：

- **`lkit self install --flare-psk-file`**：daemon 部署时在启动前写回 `[flare]` 段
  （保留既有字段）；未提供时保留既有 `[flare]`，交互终端提示输入，无终端回落
  daemon 自动生成，见 [`lkit self`](../commands/self.md)；
- **`lkit flare setup`**：带 `--psk/--token/--devices/--ethertype/--forward-ports/
  --mac/--device-name` 时在既有配置上覆盖并写回；空参打印当前有效配置（含 psk，
  供分发给 `lflare` 恢复客户端）；
- **daemon 首启**：无 `[flare]`/psk 时生成随机 psk 并写回。

`lkit install` 不写回本文件；首次安装完成时会提示 flare 恢复通道就绪
（`lkit flare setup` 查看 psk）。psK 随本文件以 `0600` 权限保存（`lkit` 写入时强制）；
文件缺失时上述动作会创建带默认 `[repository]` 的最小配置，与"文件缺失"的缺省
回退语义一致。

## 来源变化与资产身份
`lkit reconcile --repository` 与同版本 `install`/`switch` 的显式来源诊断：

- 核对显式来源清单中的 static 摘要/大小与 state 记录一致；
- 下载解压后端二进制，核对落盘身份与 state 记录一致；
- 不一致则拒绝；成功后**不保存**来源，也不修改 `config.toml`。

`lkit repair` 始终验证本次实际使用的资产：static repair 对比 state 中的 static
archive 身份，binary repair 对比解压后的后端身份；来源解析仍按上述优先级，但资产
核对不以配置文件为基准。

## 兼容性

早期 `install-state.json` 可能包含 `repository` 字段。读取时把它作为未知兼容字段忽略，
后续写入不再保留，也不会迁移到 `config.toml`；这不会改变 `schema_version`。早期版本
写入的 `state/repository.json` 不再被读取或迁移，用户可自行删除；没有 `config.toml` 的
既有安装按"未记录来源"处理，缺省官方 GitHub。
