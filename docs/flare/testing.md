# Landscape Terrain（flare）协议 e2e

flare 域验证 L2 防失联通道：`lflare` 客户端与 `lkit flare` 服务端在真实以太网
帧上的加密隧道。测试在普通 Docker 容器（bridge 网络即 L2 段）中运行，二进制从
宿主 `target/debug` 挂载，无需真实网卡；服务端防护场景需要 `NET_RAW`/`NET_ADMIN`
（tc netem、原始套接字）。

场景目录：[scenarios.md](scenarios.md)

## 入口

| 位置 | 内容 |
| --- | --- |
| `scripts/flare/e2e-docker.sh` | 单网段全功能场景（握手/传输/丢包/白名单/令牌/错误 psk/teardown/重放/重启） |
| `scripts/flare/e2e-same-segment.sh` | 同段多客户端场景（并发/优雅重启/硬杀恢复/20 MiB） |
| `scripts/flare/e2e-multiclient.sh` | 双网段多客户端场景（teardown 隔离/空闲保活） |
| `scripts/flare/e2e-ratelimit.sh` | 限速与锁死场景（洪泛/伪造失败不冻结） |
| `scripts/flare/e2e-daemon.sh` | daemon 托管形态（`lkit daemon` + config.toml `[flare]` 段托管 flare 服务端，lflare 客户端建立会话，不执行隧道转发） |
| `scripts/flare/Dockerfile` | Debian 13 slim 镜像，双模式运行时依赖：有 `packages/*.deb` 时离线 dpkg 安装（本地，`docker build` 无需网络），无 `.deb` 时（CI）apt 在线安装 `iproute2 netcat-openbsd python3`；内置 4 个测试工具（fake_service/replay_inject/rate_flood/auth_req_flood） |
| `scripts/flare/fetch-packages.sh` | 在宿主用 apt 下载 `.deb` 到 `scripts/flare/packages/`（gitignored，仅保留 `.gitkeep`），供本地离线镜像构建使用 |
| `.github/workflows/test-flare.yml` | CI：PR/push（dev、main）按 paths 过滤 + 手动触发，`cargo build --locked --workspace` 后依次运行 5 个场景脚本 |

## 场景拓扑

```
client 容器 (lflare cli --dev eth0 --forward 2222:6443)
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
| `FLR-01` 至 `FLR-18` | 见 [scenarios.md](scenarios.md) | `scripts/flare/e2e-*.sh` + `landscape-terrain-proto` 单元测试 |
| `FLR-19` 至 `FLR-21` | daemon 托管连接 / `self install` 供给 psk / 完整故障场景（缺口） | `scripts/flare/e2e-daemon.sh` + `install_fixture_e2e/self_cmd.rs` + [scenarios.md](scenarios.md) |
