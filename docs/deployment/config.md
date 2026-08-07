# 配置文件（`config.toml`）

## 职责

`<install-root>/config.toml` 是安装根目录顶层的**用户维护**配置文件，独立于
`state/install-state.json`。它保存安装机器的持续分发通道偏好，不属于安装记录：
`state/install-state.json` 不包含任何仓库信息。

`lkit` 永不创建、更新或删除 `config.toml`；该文件完全由用户编辑或删除。因此：

- 来源选择不会被任何命令持久化，成功、失败、回滚或中断恢复都不会改变该文件；
- 用户随时可以编辑或删除 `config.toml` 改变后续命令的缺省来源；
- 文件不存在与"官方 GitHub 默认"等价，首次安装不传 `--repository` 时行为一致。

`config.toml` 允许存在于安装根目录顶层（`lkit` 对顶层目录执行白名单检查，
`releases/`、`state/` 等受管目录之外的未知文件都会阻断命令）。没有该文件时首次安装
正常进行。

## Schema v1

建议权限 `0600`（由用户自行设置，`lkit` 不写入文件）。解析时未知字段和未知 section
允许并忽略。

```toml
schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"
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
  `service-manager` 迁移、`network` 子命令、`install --force`，以及不通过安装适用性
  检查就报错的命令（例如空目录上运行 `switch`/`reconcile` 或已有安装上再次
  `install`）。这些命令不受损坏配置影响。

文件**存在但损坏**（TOML 解析失败、`repository` section 或 `schema_version` 字段
缺失、字段类型错误、`schema_version` 不支持、`kind` 非法、HTTP URL 不安全或 GitHub
名称非法）或**不可读**（例如权限不足）时，读取它的命令报配置错误并阻断，提示修复
或删除 `config.toml` 以回落官方 GitHub 默认；损坏很可能来自用户编辑错误，静默回落
会让用户误以为配置仍然生效。

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
