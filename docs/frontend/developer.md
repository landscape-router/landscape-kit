# Landscape 前端开发规范（frontend developer guide）

本文定义第三方前端（web UI）如何打包、发布并被 `lkit` 集成。`lkit` 不校验前端
来源是否来自官方仓库：用户通过 `[frontend]` 配置选择前端源，完整性校验来自前端源
自身的发布元数据，结构安全检查与来源无关、永远强制。

> **信任警告**：自定义前端与官方页面由同一个 Landscape webserver 以同一 TLS 证书在
> 同一源（`https://<landscape>/`）提供服务，可调用完整的 `/api/v1/*` 管理 API（含
> 认证上下文）。恶意前端等于完全接管 Landscape 的管理面。结构校验与 SHA-256 只保证
> "包未被传输途中篡改"，不保证"作者可信"。**用户选择前端源 = 自行承担信任责任**。

## 包格式

前端发布物是一个 zip 压缩包，内部结构与官方 `static.zip` 完全同规格：

```text
static/
├── index.html        # 必选入口
├── assets/           # 静态资源（js/css/图片等）
├── frontend.json     # 可选元数据
└── …                 # 普通静态内容
```

lkit 侧强制校验（与来源无关）：

- 所有有效条目必须位于 `static/` 前缀下；
- 必须包含普通文件 `static/index.html`；
- 只允许目录与普通文件：拒绝符号链接、设备文件、特殊文件；
- 拒绝绝对路径、`..` 穿越、`\` 分隔符、盘符前缀、重复规范化路径；
- 解压总字节数不得超过压缩包声明大小的 `20` 倍与 `1 GiB` 中较小者。

## 可选元数据 `static/frontend.json`

```json
{
  "name": "社区前端",
  "version": "2.3.1",
  "api_min_version": "0.19.0",
  "author": "community"
}
```

- `name`：展示名，`lkit frontend status` 显示。
- `version`：前端自己的版本号，**仅信息性**，不参与任何解析与匹配。
- `api_min_version`：声明的后端 API 兼容下限；lkit 解析到当前后端版本低于该值时
  输出警告（不阻断）。版本锁步模型下该声明由前端作者负责，属于双保险。
- `author`：信息性。

由于元数据位于 zip 内部，随包 SHA-256 一起被校验，作者的声明是"绑定的"。

## 运行时契约

- 页面入口 `index.html` 在 `https://<landscape-host>/` 根路径提供（webserver 以
  `--web <root>/current/static` 启动，TLS 证书与端口由 lkit 管理）；
- 所有资源必须使用**相对路径**（`./assets/app.js`），不得依赖绝对 URL；
- 管理 API 走 `/api/v1/*`；接口契约以 webserver 的 OpenAPI 文档
  （`GET /api/docs`）为准；
- 健康检查路径 `/api/docs` 是 v1 固定且稳定的契约，前端不得占用或改写；
- 前端只允许普通静态内容，lkit 不允许 zip 内含服务端可执行逻辑（符号链接、
  设备文件等一律拒绝）。

## 发布协议（前端源）

前端源与后端发布仓库同协议，但**只提供 static 资产**（不要求 webserver 二进制）。

### GitHub 形式

源位置为 `owner/repo`。规范要求：

1. 仓库的 **latest release** 始终是当前维护的最新版本（作者手动 "Set as latest"）；
2. latest release 必须携带 `static.zip` 与 `SHASUM256sum.txt`（GNU `sha256sum`
   文本格式，内容含 `static.zip` 的 SHA-256）；
3. `SHASUM256sum.txt` 内 `static.zip` 的摘要即下载校验基准；
4. release 不得为 draft 或 prerelease。

`lkit` 解析流程：

1. `GET /repos/{owner}/{repo}/releases/latest`；
2. 解析该 release 的 `static.zip` + `SHASUM256sum.txt` 资产；
3. 按 `SHASUM256sum.txt` 校验下载的 `static.zip`（大小 + SHA-256）。

前端作者在发布新版本时**始终更新 latest release**：旧版本的修复也通过发布新
release 并设为 latest 完成（GitHub 资产不可覆盖，不得在既有 release 上修改资产）。

### HTTP 形式

源位置为 protocol v1 仓库 base URL。规范要求该仓库提供：

```text
<base>/repository.json
<base>/channels/stable.json
<base>/releases/<version>/manifest.json
```

其中 `manifest.json` 的 `webserver` 为空对象，只声明 `assets.static`：

```json
{
  "protocol_version": 1,
  "version": "2.3.1",
  "assets": {
    "webserver": {},
    "static": { "url": "static.zip", "sha256": "<64 hex>", "size": 12345 }
  }
}
```

`lkit` 解析流程：`repository.json`（协议校验）→ `channels/stable.json`（stable
版本指针）→ `releases/<version>/manifest.json` → 按 `assets.static` 的
sha256 + size 校验下载。HTTP 形式没有版本枚举能力，每个 stable 指针对应一个
不可变前端包；修复/新版本通过发布新的 stable 指针完成。

## 校验模型

| 层 | 内容 | 来源 |
|---|---|---|
| 完整性 | 包 SHA-256 + 大小 | 前端源自身元数据（`SHASUM256sum.txt` / `manifest.json`），HTTPS 传输 |
| 结构安全 | 路径、条目类型、解压上限 | lkit 代码内强制，与来源无关 |
| 兼容性 | `api_min_version` 警告 | zip 内 `frontend.json`，随包校验，不阻断 |

## 版本模型

- lkit 解析前端源的 **latest/stable**，不与后端版本号匹配；
- 前端作者负责保持 latest/stable 与当前维护的 Landscape 后端兼容；
- 用户锁定旧后端版本时，自定义前端可能与其不兼容——由 `api_min_version` 警告
  提示，用户自行承担；
- 源不可达或元数据非法时，需要前端源的命令**阻断**并提示逃生路径（移除
  `[frontend]` 配置或 `lkit frontend select official`）。

## 与 lkit 的集成行为

用户通过 `config.toml` 的 `[frontend]` 段登记多个前端源并选择激活项（见
[配置文件](../deployment/config.md)）。不配置或激活 `official` = 官方页面。

| 场景 | 行为 |
|---|---|
| install / update / switch | 用官方 `static.zip` 构建版本目录后，按激活源解析 latest/stable 并下载校验解压，原子替换 `releases/<version>/static/`；源不可达 = 整个命令阻断 |
| `repair static` | 激活源非官方时重新拉取该前端源；否则恢复官方页面；`--official` 强制恢复官方 |
| backup | 从 `current/static/` 现场打包 `static.zip` 入 `.lkb`（含自校验） |
| restore | 不校验前端身份，恢复内容即备份快照 |
| `reconcile --repository` | 配置了自定义前端时跳过 static 身份核对（只核对后端二进制） |
| 版本目录 `static.zip` | 始终保持官方基线不变，自定义前端只影响 `static/` 目录内容 |

## 发布指引（前端作者）

1. 构建 zip：单一顶层目录 `static/`，包含 `index.html` 与 `frontend.json`；
2. 计算 SHA-256：`sha256sum static.zip`；
3. GitHub 形式：为 zip 与 `SHASUM256sum.txt` 创建 GitHub Release（tag 命名任意，
   建议用前端自己的版本号 `v2.3.1`），并**设为 latest**；
4. HTTP 形式：按 [发布仓库协议](../repository.md) 发布 `manifest.json` 与
   `static.zip`，并更新 `channels/stable.json` 指向该版本；
5. 在 `frontend.json` 中如实声明 `api_min_version`，保持 latest/stable 与当前
   维护的 Landscape 后端兼容；
6. 发布工具自动化（自动更新映射/指针）为后续增强，v1 按上述手工流程执行。
