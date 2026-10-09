# 自定义部署

`lkit custom` 从本地目录部署用户提供的后端与静态页面，是受控部署通道而不是合法化
通道。合法性模型、目录契约与命令行为见
[`lkit custom`](../../../commands/custom.md) 与
[后端合法性与主线对照](../../../deployment/backend-legality.md)。

## CUS-01

**目录契约校验：非法目录在创建事务前拒绝**

- 测试层：Rust workflow/CLI
- 状态：`待补充`
- 证据：[目录契约](../../../commands/custom.md#目录契约)
- 说明：缺 `landscape-webserver`、`static/index.html` 或 `release.toml`，`version`
  非 canonical stable SemVer，任意条目为符号链接、非普通文件/目录，或二进制为空时，
  返回参数错误 `2`，不创建事务、不写任何文件、不修改安装现场。

## CUS-02

**已安装机器升级到自定义构建（更高版本）**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[已安装分支](../../../commands/custom.md#已安装分支)
- 说明：目标版本高于当前版本时走 switch 流水线：保护 `.lkb`（如实快照 + backend
  标注）、事务、健康检查、失败自动回滚全部与 switch 一致；提交后 `sha256` 锚定到
  部署目录的二进制。

## CUS-03

**未安装机器从本地目录全新安装**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[未安装分支](../../../commands/custom.md#未安装分支)
- 说明：无有效安装状态时走完整 install 流水线（初始化配置、凭据、unit 注册、健康
  检查），资产来自本地目录；版本只要求合法，无"必须更高"限制。

## CUS-04

**联网对照与主线一致：安装后即合法（离线官方安装场景）**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[合法性判定](../../../commands/custom.md#合法性判定)、[对照方式](../../../deployment/backend-legality.md#主线与对照方式)
- 说明：本地目录的二进制与主线落盘形态哈希一致（GitHub 读 `SHASUM256sum.txt`、
  HTTP 读 `sha256_decompressed`）时，`official_sha256` 记录主线哈希，安装后状态为
  `official`，`update`/`switch` 不被门槛阻断。

## CUS-05

**离线或版本不在主线：对照身份为空、状态 custom**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[合法性判定](../../../commands/custom.md#合法性判定)
- 说明：对照来源不可达或主线不存在该版本时部署照常完成，`official_sha256` 为
  `null`，状态 `custom`；事后 `lkit repair binary` 可补做对照（一致转正，不一致
  替换，见 [REP-07](repair.md#rep-07)/[REP-08](repair.md#rep-08)）。

## CUS-06

**完整性漂移状态下 custom 部署需要显式确认**

- 测试层：Rust workflow、CLI
- 状态：`待补充`
- 证据：[已安装分支](../../../commands/custom.md#已安装分支)
- 说明：磁盘二进制与 state 锚点不一致（`drifted`）时默认拒绝；交互模式经后端替换
  确认、非交互携带 `--accept-drift` 才能继续；拒绝返回 `1` 且零副作用。

## CUS-07

**平级与更低版本拒绝执行**

- 测试层：Rust workflow
- 状态：`待补充`
- 证据：[已安装分支](../../../commands/custom.md#已安装分支)
- 说明：已安装分支要求 `release.toml` 版本严格高于当前活动版本；相同或更低版本
  返回参数错误 `2`，不创建事务、不打包资产。
