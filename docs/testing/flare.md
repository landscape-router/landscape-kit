# Landscape Terrain（flare）协议 e2e

flare 域验证 L2 防失联通道：`lflare` 客户端与 `lkit flare` 服务端在真实以太网
帧上的加密隧道。测试在普通 Docker 容器（bridge 网络即 L2 段）中运行，二进制从
宿主 `target/debug` 挂载，无需真实网卡；服务端防护场景需要 `NET_RAW`/`NET_ADMIN`
（tc netem、原始套接字）。

场景目录：[functional/flare.md](scenarios/functional/flare.md)

## 入口

| 位置 | 内容 |
| --- | --- |
| `scripts/test-flare.sh` | 单入口：构建 → 依次运行 4 个场景（single-segment、same-segment、multiclient、ratelimit） |
| `scripts/docker/flare/Dockerfile` | Debian slim 镜像：离线安装 `libpcap0.8 iproute2 netcat-openbsd python3`，内置 4 个测试工具（fake_service/replay_inject/rate_flood/auth_req_flood） |
| `scripts/docker/flare/fetch-packages.sh` | 在宿主用 apt 下载 `.deb` 到 `packages/`（gitignored），使 `docker build` 离线可用 |
| `.github/workflows/test-flare.yml` | CI：PR/push（dev、main）按 paths 过滤 + 手动触发，`cargo build --locked --workspace` 后运行脚本 |

## 场景拓扑

```
client 容器 (lflare --dev eth0 --forward 2222:6443)
     |  Terrain 帧 (ethertype 0x88B6, broadcast + unicast)
     v
server 容器 (lkit flare serve --dev any, fake service on 127.0.0.1:6443)
```

## 断言依赖的日志契约

脚本通过容器日志断言协议行为，改动下列输出时必须同步更新场景断言：

- 客户端 `session … established`（会话建立，`wait_session`/`wait_sessions`）
- 服务端 `discover … ignored (cannot open)`（错误 psk 静默拒绝）
- 服务端 `token mismatch`（发现令牌不匹配）
- 服务端 `sent teardown`（优雅断开）
- 服务端 `rate-limited` / `auth rejected for` / `ignored (lockout)`（防护计数）
- 客户端 `captured … injected …`（replay 工具输出）

## 场景覆盖

| 场景 ID | 场景 | 位置 |
| --- | --- | --- |
| `FLR-01` 至 `FLR-18` | 见 [functional/flare.md](scenarios/functional/flare.md) | `scripts/test-flare.sh` + `landscape-terrain-proto` 单元测试 |
