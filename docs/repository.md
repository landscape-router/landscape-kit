# `install::repository`：发布仓库与资产下载

本文定义 `install` 的数据源、统一发布模型、第三方 HTTP 仓库协议、发布流程和资产准备规则。规范与对应实现放在同一目录。

```text
repository/
├── repository.md
├── mod.rs
├── github.rs
├── http.rs
└── download.rs
```

- `mod.rs` 定义统一发布模型、provider 接口和版本选择；
- `github.rs` 读取官方 GitHub Releases；
- `http.rs` 读取第三方静态仓库；
- `download.rs` 负责共享的网络请求、重试、流式下载、摘要校验和压缩包解压。

## 数据源边界

v1 支持两种数据源：

- 第一方数据源是官方 GitHub Releases；
- 第三方数据源是符合本文协议的公开 HTTP 静态仓库。

第三方仓库可以托管在 S3 兼容对象存储中，但对象存储只作为静态文件服务。仓库维护者通过 GitHub Actions 发布资产；`lkit` 不实现 S3 profile、region、access key、签名请求或凭据链，只通过公开 HTTPS 地址读取仓库。

两个 provider 必须转换为相同的内部发布模型。版本选择和安装流程不得根据 provider 类型产生不同语义。

## 第三方 HTTP 仓库协议 v1

### 目录结构

```text
<base-url>/
├── repository.json
├── channels/
│   └── stable.json
└── releases/
    └── 0.19.2/
        ├── manifest.json
        ├── landscape-webserver-x86_64.zst
        ├── landscape-webserver-aarch64.zst
        └── static.zip
```

职责如下：

- `repository.json` 是固定的仓库协议入口；
- `channels/stable.json` 是可变的 stable 版本指针；
- `releases/<version>/manifest.json` 描述一个不可变版本；
- `releases/<version>/` 中的 manifest 和资产公开后不可覆盖；
- v1 不提供全量版本索引和版本枚举接口。

首次安装未指定 `--repository` 时使用默认 GitHub provider
`ThisSeanZhang/landscape`。已有安装的管理命令未指定该参数时沿用 state 记录的仓库。
仅指定 `--repository` 而不传值时使用默认 HTTP 镜像
`https://l1s3.whileaway.dev/landscape/`；传入值时使用指定的 HTTP base URL。显式参数
就是本次操作使用该来源的授权，不要求额外确认。

`--repository [base-url]` 禁止 query、fragment 和 URL 用户信息。HTTPS 可用于任意主机；HTTP 只允许 `localhost`、`127.0.0.1` 和 `[::1]`。规范化时保留路径前缀并补齐目录语义的结尾 `/`。非法 base URL 在网络请求前失败。

### 根描述文件

固定入口：

```text
<base-url>/repository.json
```

Schema v1：

```json
{
  "protocol_version": 1
}
```

| 字段 | 类型 | 规则 |
| --- | --- | --- |
| `protocol_version` | integer | 必填，v1 固定为 `1` |

缺少文件、字段缺失、类型错误或协议版本不支持时拒绝整个仓库。未知字段允许并忽略。

根描述文件公开后保持不变，可使用长期 immutable HTTP 缓存。它用于区分有效但尚未发布 stable 版本的仓库和错误 URL。

### Stable 指针

固定路径：

```text
<base-url>/channels/stable.json
```

Schema v1：

```json
{
  "protocol_version": 1,
  "version": "0.19.2"
}
```

| 字段 | 类型 | 规则 |
| --- | --- | --- |
| `protocol_version` | integer | 必填，v1 固定为 `1` |
| `version` | string | 必填，不带 `v` 的规范化 stable SemVer |

规则：

- 文件不存在表示仓库有效但当前没有 stable 版本；
- 默认安装和 `--version latest` 读取该指针；
- 指针不能引用 prerelease；
- 指针只能单向推进到更高 SemVer，不得因发布历史版本而降级；
- 指针允许覆盖，必须使用 `Cache-Control: no-cache`；
- 发布流程最后更新该文件，使新 stable 版本一次性对安装器可见。

v1 不定义 prerelease、beta 或其他 channel 指针。

### 版本 Manifest

固定路径：

```text
<base-url>/releases/<version>/manifest.json
```

显式版本安装直接请求该路径，不需要读取 stable 指针或枚举历史版本。

Schema v1：

```json
{
  "protocol_version": 1,
  "version": "0.19.2",
  "assets": {
    "webserver": {
      "x86_64": {
        "url": "landscape-webserver-x86_64.zst",
        "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "size": 12345678
      },
      "aarch64": {
        "url": "landscape-webserver-aarch64.zst",
        "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        "size": 12345678
      }
    },
    "static": {
      "url": "static.zip",
      "sha256": "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
      "size": 2345678
    }
  }
}
```

根字段：

| 字段 | 类型 | 规则 |
| --- | --- | --- |
| `protocol_version` | integer | 必填，v1 固定为 `1` |
| `version` | string | 必填，不带 `v` 的规范化 stable SemVer |
| `assets` | object | 必填 |

规则：

- manifest 中的 `version` 必须与请求路径中的规范化版本完全一致；
- 非法 SemVer、prerelease、字段缺失或类型错误导致 manifest 无效；
- manifest 和同目录资产公开后不可覆盖；
- 未知字段允许并忽略；
- 显式版本 manifest 不存在表示该仓库没有该版本。

### 资产结构

`assets.webserver` 是架构名到资产对象的映射。v1 识别 `x86_64` 和 `aarch64`；未知架构字段忽略。

一个版本可以只提供其中一个架构。缺少当前主机架构资产不算 manifest 损坏，但该版本不能安装到当前主机。

`assets.static` 是所有架构共享的静态页面压缩包。

每个资产对象必须包含：

| 字段 | 类型 | 规则 |
| --- | --- | --- |
| `url` | string | 必填，相对或绝对安全 URL |
| `sha256` | string | 必填，64 个小写十六进制字符 |
| `size` | integer | 必填，取值范围为 `1..=u64::MAX`，单位为字节 |

JSON 数值必须能无损转换为 `u64` 正整数。实际下载大小或 SHA-256 不一致时立即失败。

### URL 解析与安全

- manifest 中的相对 URL 以该 manifest 所在版本目录为基准；
- 相对 URL 不得通过 `..` 逃出 `releases/<version>/`；
- 绝对 URL 只允许 HTTPS；
- 明文 HTTP 只允许 `localhost`、`127.0.0.1` 和 `[::1]`；
- 禁止 `file:`、`ftp:`、`data:` 及其他协议；
- URL 不得包含用户名或密码；
- 最多允许 5 次重定向，最终 URL 仍须通过安全校验；
- 日志展示 URL 时必须移除 query 和 fragment。

### 网络超时与重试

第三方仓库和 GitHub provider 共用以下策略：

- DNS 解析与 TCP/TLS 建连单次超时 `10` 秒；
- 根描述、channel、manifest、Release 元数据和校验清单单次请求总超时 `60` 秒；
- 资产下载总超时 `30` 分钟；
- 接收响应体期间连续 `30` 秒没有收到任何字节视为超时；
- 单个仓库元数据响应体上限固定为 `10 MiB`；
- GitHub latest、tag 和单个 Release 元数据响应体上限固定为 `10 MiB`；
- `SHASUM256sum.txt` 响应体上限固定为 `10 MiB`；
- 每个请求最多尝试 `3` 次，包括首次请求；
- 可重试错误只包括 DNS/连接失败、连接重置、超时、HTTP `408`、`429` 和 `5xx`；
- 其他 `4xx` 不重试；
- 普通重试等待 `1` 秒、`2` 秒并加入不超过 `250` 毫秒的随机抖动；
- 每次资产重试都删除不完整临时文件并从头下载，v1 不实现 Range 续传；
- `Retry-After` 不超过 `60` 秒时优先等待，超过时立即返回包含建议重试时间的错误；
- 达到最大尝试次数后失败，不进入激活阶段。

所有响应体必须在读取过程中执行大小限制，不能先无界加载到内存。

## 官方 GitHub Releases Provider

官方 provider 只使用 GitHub REST API，不解析 GitHub HTML 页面。默认仓库固定为 `ThisSeanZhang/landscape`。

请求规则：

- 解析 `latest` 时调用 `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`；
- 请求头至少包含 `Accept: application/vnd.github+json`、非空 `User-Agent` 和 `X-GitHub-Api-Version: 2022-11-28`；
- 公共仓库默认允许匿名请求；存在 `GITHUB_TOKEN` 时作为 Bearer token 使用，但不得写入日志、状态、事务或错误详情；
- 解析 `latest` 时直接复用该接口返回的 Release 资产，不遍历分页列表，也不再次按 tag 查询；
- latest Release 的 tag 不是规范化 stable SemVer（可带单个 `v` 前缀）时，视为该仓库没有可安装的 stable 版本；
- 显式版本同时接受 tag `0.19.2` 和 `v0.19.2`，两者同时存在时视为冲突；
- `404` 不推断仓库、权限或版本中的具体原因；
- `401` 表示 token 无效；
- `403`/`429` 按 GitHub 速率限制响应头处理；
- 其他状态按通用网络规则处理。

Release 资产使用元数据中的 `browser_download_url` 下载。`SHASUM256sum.txt` 使用 GNU `sha256sum` 文本格式严格解析。

官方 provider 转换为统一发布模型时：

- 忽略 draft 和 prerelease；
- 使用 SemVer 排序；
- 同一版本的后端、静态页面和校验清单必须来自同一个 Release；
- `x86_64` 原始资产使用 `landscape-webserver-x86_64`；
- `aarch64` 原始资产使用 `landscape-webserver-aarch64`；
- 共享静态资产使用 `static.zip`；
- SHA-256 从 `SHASUM256sum.txt` 读取；
- v1 Debian 安装不选择 `-musl` 或实验架构资产。

## 后端压缩格式

第三方镜像仓库中的后端固定使用 Zstandard 单文件格式：

```text
landscape-webserver-x86_64.zst
landscape-webserver-aarch64.zst
```

manifest 中的 `size` 和 `sha256` 描述压缩后的 `.zst` 对象。

发布流程必须先按官方 `SHASUM256sum.txt` 校验原始后端，再使用 `zstd --ultra -19` 生成压缩资产。安装器按以下顺序处理：

1. 流式下载 `.zst` 到事务临时文件；
2. 校验压缩文件的声明大小和 SHA-256；
3. 使用受限流式解压，拒绝尾随数据和损坏帧；
4. 解压为临时的 `landscape-webserver`；
5. 校验文件类型、目标架构和可执行格式；
6. 设置受管权限后原子放入版本目录。

解压后的最大字节数不得超过 `1 GiB`。`.zst` 只用于传输和镜像存储，最终版本目录保存无压缩后端。

## 下载与发布目录

所有资产先下载到本次事务专用临时目录。只有压缩资产完整通过校验并安全解压后，才能进入 `releases/<version>`。

校验至少包括：

1. HTTP 请求成功且响应完整；
2. 实际大小与声明 `size` 一致；
3. SHA-256 与可信仓库元数据一致；
4. 后端 `.zst` 可安全解压，解压结果是当前架构可执行文件；
5. `static.zip` 可解压并包含预期静态目录；
6. 压缩包不存在绝对路径、`..` 穿越、设备文件或逃逸链接。

`static.zip` v1 内部结构固定为单一顶层目录 `static/`：

```text
static/
├── index.html
├── assets/
└── 可选的 scalar/ 等普通静态内容
```

ZIP 的所有有效条目必须位于 `static/` 前缀下。去掉该前缀后解压到事务临时目录，最终原子移动为 `releases/<version>/static/`。只允许目录和普通文件；解压总字节数不得超过压缩资产声明大小的 `20` 倍和 `1 GiB` 中较小者。

下载、校验或解压失败不得停止当前服务、修改 `current` 或更新成功状态。

目标版本目录已存在时：

- 目录必须位于本安装根目录内且不是符号链接；
- 解压后的后端必须与 manifest 描述的可信压缩资产对应，并通过架构和可执行格式检查；
- `static/` 必须是普通目录并至少包含普通文件 `index.html`；
- 满足规则时可复用现有目录；
- 不可信或残缺目录立即阻断，v1 不自动删除、隔离或覆盖。

## 静态页面策略

安装后不逐文件校验 `static/`：

- 下载阶段仍校验 `static.zip` 的大小和 SHA-256；
- 普通同版本检查不验证、不覆盖静态目录；
- 静态目录缺失时报告 warning；
- 用户可直接修改某版本目录中的 `static/`；
- 版本切换使用目标版本发布资产创建的静态目录；
- 切回旧版本时继续使用旧版本目录中保留的页面；
- `lkit repair static` 重新下载目标版本 `static.zip`，备份当前目录后恢复发布版页面。

## 第三方仓库发布流程

本仓库通过 [`.github/workflows/publish-landscape-mirror.yml`](../.github/workflows/publish-landscape-mirror.yml) 手动镜像指定的官方 stable Release，并使用独立的 `lkit-publish` 二进制执行发布。普通 `lkit` 用户不需要安装发布工具。

发布顺序固定为：

1. 校验稳定 SemVer、两个 `.zst` 后端和 `static.zip`；
2. 计算每个资产的大小与 SHA-256；
3. 校验或以条件写入初始化不可变的 `repository.json`；
4. 确认目标版本的 manifest 不存在；
5. 以条件写入上传两个 `.zst` 后端和 `static.zip`；
6. 最后以条件写入上传不可变的 `releases/<version>/manifest.json`；
7. 如果目标版本高于当前 stable，使用 ETag 条件更新 `channels/stable.json`；
8. 如果目标版本低于当前 stable，只发布历史版本，不移动 stable 指针。

资产和 manifest 使用长期 immutable 缓存；stable 指针使用 `no-cache`。发布工具通过签名 S3 API 读取仓库状态，不依赖 CDN 缓存结果。

GitHub Actions 使用固定 concurrency group 降低并发发布概率，发布工具仍通过 `If-None-Match` 和 ETag `If-Match` 防止重复对象或并发 stable 更新。Access Key 和 Secret Access Key 只能来自 GitHub Actions secrets 或本地环境，不得写入 workflow、脚本、日志或仓库文件。

发布失败时：

- 已上传但尚未拥有 manifest 的对象对安装器不可见；
- manifest 未成功上传时不得更新 stable；
- stable 更新失败时该版本仍可通过显式版本安装；
- 重复版本发布必须失败，不允许覆盖任何版本对象。

## 测试分层

仓库实现使用分层测试，不允许普通单元测试依赖共享的公网 RustFS、Cloudflare、DNS 或长期凭据。

### 单元测试

`cargo test` 中的单元测试只验证纯逻辑和内存中的 JSON，不发起网络请求。至少覆盖：

- `repository.json`、`stable.json` 和版本 manifest Schema；
- 协议版本、规范化 SemVer 和 stable/prerelease 规则；
- 版本路径与 manifest 版本一致性；
- 架构选择、资产大小和 SHA-256；
- URL 规范化、相对路径逃逸和不安全协议；
- 缺失字段、非法类型和未知字段兼容。

测试数据直接写在测试代码中或存放于仓库内的小型 fixture，不从 `landscape-test` 下载。单元测试必须能够离线、并行和无凭据运行。

### 本地 HTTP 集成测试

HTTP provider 的集成测试使用绑定到 `127.0.0.1` 随机端口的临时 HTTP Server，并提供以下 fixture：

```text
repository.json
channels/stable.json
releases/1.2.3/manifest.json
```

该层调用真实的 `HttpRepository::latest` 和显式版本读取接口，至少验证：

- 请求路径和请求顺序；
- 仓库无 stable 时的行为；
- manifest 不存在、非法 JSON 和非 `200` 状态；
- Content-Length 和实际响应体上限；
- 安全与不安全重定向；
- 截断响应和连接失败。

本地 HTTP 集成测试不需要 S3，因为安装器只通过公开 HTTP 接口消费第三方仓库。

### RustFS 容器集成测试

`lkit-publish` 的 S3 行为使用临时 RustFS 容器验证。测试入口固定为：

```text
scripts/test-publish-http-repository.sh
```

测试必须固定 RustFS 镜像版本和 digest，不使用浮动的 `latest`。当前 CI 固定 `rustfs/rustfs:1.0.0-beta.11` 对应镜像摘要；容器只绑定本机回环地址，使用专用测试 Access Key、Secret Key、bucket 和临时 Docker volume。测试结束时无论成功或失败都删除容器、volume 与临时文件。

容器测试至少执行：

1. 启动 RustFS 并等待 S3 API 可用；
2. 创建测试 bucket 并配置匿名 `GetObject`；
3. 生成小型 `.zst` 后端和 `static.zip`；
4. 运行真实的 `lkit-publish` 二进制；
5. 验证根描述、manifest、资产和 stable 指针；
6. 发布更高版本并确认 stable 前进；
7. 发布较低版本并确认 stable 不降级；
8. 重复发布同一版本并确认拒绝覆盖；
9. 模拟发布失败并确认 manifest 和 stable 不提交。

该测试属于集成测试，不默认混入普通 `cargo test`。本地未设置 `RUSTFS_TEST_REQUIRE` 时可以在 Docker 不可用时明确跳过；专用 GitHub Actions job 必须设置 `RUSTFS_TEST_REQUIRE=1`，Docker 或镜像不可用时直接失败，不允许以跳过状态误报成功。

### 公网 Smoke Test

`landscape-test` 只用于验证真实部署链路：

- RustFS 的实际 S3 API 和 bucket policy；
- GitHub Actions secrets 注入；
- HTTPS、DNS 和证书；
- Cloudflare 代理、缓存和 404 负缓存；
- 匿名下载和 S3 Signature V4 上传。

公网 smoke test 通过手动 workflow 或定时 workflow 串行执行，不属于每个 pull request 的必需测试，也不允许成为 `cargo test` 的前置条件。测试使用临时版本对象，完成后必须删除资产、manifest 和 stable 指针，并将测试仓库恢复为只包含有效 `repository.json` 的空仓库。

`landscape-test` 不保存供单元测试使用的永久版本 fixture。共享公网状态可能被人工操作、网络故障或缓存影响，只能用于验证部署环境，不能作为代码逻辑测试的唯一依据。

## 验收标准

- 缺少或损坏 `repository.json` 时拒绝仓库。
- 缺少 `channels/stable.json` 时默认安装报告没有 stable 版本，显式版本仍可安装。
- 默认安装只读取根描述、stable 指针和目标 manifest，不读取历史版本列表。
- 显式版本直接读取固定 manifest，manifest 不存在时报告版本不可用。
- manifest 版本与路径不一致、协议错误、非法 SemVer、摘要或大小非法时拒绝。
- 相对 URL 逃出版本目录、非法协议、凭据 URL 或不安全重定向被拒绝。
- 缺少当前架构资产时报告版本与主机不兼容。
- 发布首个版本时创建根描述、manifest 和 stable 指针。
- 发布更高版本时推进 stable，发布较低版本时 stable 不降级。
- 重复发布不覆盖 manifest 或资产。
- 资产上传或校验失败不会创建 manifest 或更新 stable。
- `.zst` 后端和 `static.zip` 均在修改当前运行状态前完成下载和完整性校验。
