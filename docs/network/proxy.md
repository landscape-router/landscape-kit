# 网络出口与代理语义

## 职责

规定 lkit 全部出站 HTTP 请求对代理环境变量（`http_proxy`/`https_proxy`/`all_proxy`/`NO_PROXY`，
大小写变体均识别）的处理规则：哪些请求遵循代理、哪些必须直连、直连如何判定。

## 三个具名 client 角色

| 角色 | 实现位置 | 语义 |
|---|---|---|
| 外部下载 | `release/repository/download`（`DownloadClient`） | 按目标 host 分流 |
| 外部镜像探测 | `mirror/availability` | 按目标 host 分流 |
| 内部验证 | `service/health`（`HttpsDocsProbe`）、`backup/export`（`export_config`） | 恒直连 |

host 字面量判定共用 `lkit-cli/src/proxy.rs` 的 `is_loopback_host`。

## 分流规则（外部角色）

每个外部 client 内部持有两条通道，发出请求前按请求 URL 的 host 字面量判定：

- host 为 `localhost`、`127.0.0.1`、`[::1]`（与仓库协议允许明文 HTTP 的回环集合一致，
  见[发布仓库协议](../repository.md)的「URL 解析与安全」）→ 直连通道，
  不读取任何代理环境变量；
- 其余 host → 代理通道，遵循标准代理环境变量（含 `NO_PROXY` 排除列表）。

判定基于 URL 字面量，不做 DNS 解析。重定向在发起请求的通道内跟随：下载通道每次跳转
仍须通过 URL 安全校验，且跳转目标的回环性必须与通道一致——外部通道不得借重定向进入
回环，直连通道不得经重定向发往外部；跨界跳转被拒绝，`3xx` 响应原样返回上层按错误
处理。镜像探测通道的重定向同样不跨回环边界。

## 内部验证角色

目标恒为本机 Landscape 服务（`https://127.0.0.1:6443` 上的 `/api/docs` 健康探测与
`/api/v1/system/config/export` 配置导出）。这类 client 构建时即禁用代理，代理环境变量
对其完全无效，也不参与分流。

## 为什么回环必须无条件直连

设置了代理环境变量的 shell 会把发往 `127.0.0.1` 的请求劫持进代理：本地代理通常按规则
拒绝或错误处理回环目标，远程代理则把 `127.0.0.1` 解析到代理自身所在主机。两种情况都会
使服务健康验证与本地 HTTP 仓库访问误报失败。因此回环直连是无条件的，不依赖用户正确
设置 `NO_PROXY`。

## 相关文档

- [发布仓库协议](../repository.md)：下载 URL 安全规则与重试策略；
- [服务、进程与健康检查](../service/runtime-and-health.md)：`/api/docs` 健康检查流程；
- [`.lkb` 备份与回滚](../backup/lkb-and-rollback.md)：配置导出 API；
- [`set-mirror`](../commands/mirror.md)：镜像可用性探测。
