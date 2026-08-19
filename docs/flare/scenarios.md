# Landscape Terrain（flare）协议场景

## FLR-01

**握手与加密数据传输完整性**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（single-segment base transfer）、[协议规范](protocol.md)
- 说明：2 MiB 随机数据经隧道往返，md5 一致。

## FLR-02

**丢包链路上的传输完整（smoltcp 重传）**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（tc netem 10% loss）
- 说明：服务端 eth0 注入 10% 随机丢包后重传，md5 一致。

## FLR-03

**转发白名单：未允许端口被关闭**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（whitelist 场景）

## FLR-04

**发现令牌反扫描：无令牌客户端得不到会话**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（anti-scanning 场景）

## FLR-05

**错误 psk 静默拒绝（sealed DISCOVER）**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（wrong-psk 场景）
- 说明：DISCOVER 用 psk 派生的发现前密钥密封，错误 psk 客户端在发现阶段即被拒绝，
  服务端保持静默。

## FLR-06

**teardown 后服务端立即断开会话**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（teardown 场景）

## FLR-07

**重放帧被拒绝，传输不受影响**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（replay injection 场景）
- 说明：`replay_inject.py` 捕获隧道帧后原样重放，会话反重放窗口拒绝旧序号。

## FLR-08

**服务端重启后客户端自动重连**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-docker.sh)（server restart 场景）

## FLR-09

**同段多客户端并发传输互不干扰**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-same-segment.sh)（same-segment 场景）

## FLR-10

**优雅重启后同 MAC 客户端重连**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-same-segment.sh)（same-segment 场景）

## FLR-11

**硬杀后旧会话被新握手替换**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-same-segment.sh)（same-segment 场景）

## FLR-12

**持续大流量传输（20 MiB）**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-same-segment.sh)（same-segment 场景）

## FLR-13

**双网段多客户端（`--dev any` 跨接口路由回复）**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-multiclient.sh)（multiclient 场景）

## FLR-14

**teardown 隔离：一个客户端断开不影响另一个**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-multiclient.sh)（multiclient 场景）

## FLR-15

**空闲会话超过 stale 清扫窗口仍存活（keepalive 回显）**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-multiclient.sh)（multiclient 场景）
- 说明：50 秒空闲长于服务端 45 秒 stale 窗口，keepalive 保持对端存活。

## FLR-16

**DISCOVER 洪泛被限速，伪造 MAC 无法认证**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-ratelimit.sh)（ratelimit 场景）、[服务端防护](protocol.md#服务端防护)

## FLR-17

**伪造认证失败不冻结活动会话**

- 测试层：flare e2e
- 状态：`已覆盖`
- 证据：[flare e2e 脚本](../../scripts/flare/e2e-ratelimit.sh)（ratelimit 场景）
- 说明：12 次伪造 AUTH_REQ 后硬杀重连，受害 MAC 未进锁死名单。

## FLR-18

**密钥派生、握手状态机与反重放（协议级单元）**

- 测试层：Rust 单元测试
- 状态：`已覆盖`
- 证据： [crypto 测试](../../landscape-terrain-proto/src/protocol/crypto.rs)、
   [session 测试](../../landscape-terrain-proto/src/protocol/session.rs)
- 说明：scrypt 派生稳定性、方向密钥独立、握手 proof 不泄露会话密钥、错方向/篡改/
   重放/回绕窗口、伪造 RESP 拒绝、错误 psk DISCOVER 静默。

## FLR-19

**daemon 托管形态下客户端可连接（不执行隧道转发）**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[e2e-daemon.sh](../../scripts/flare/e2e-daemon.sh)
- 说明：服务端容器运行 `lkit daemon`（`LKIT_TERRITORY` + config.toml `[flare]` 段），
  由 daemon 托管 flare 服务端；`lflare` 客户端完成 DISCOVER→RESP→AUTH_REQ→AUTH_ACK
  并建立会话、保持 keepalive 即通过。本场景只验证"daemon 托管方式下能连上"，
   不做端口转发与隧道数据传输。

## FLR-20

**daemon 部署（`lkit self install`）供给急救恢复码并写入 `[flare]` 配置段**

- 测试层：fixture e2e（`install_fixture_e2e`，CI）
- 状态：`已覆盖`
- 证据：[self_cmd.rs](../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)（`[flare]` 段断言）、
   [self.md](../../commands/self.md)、[协议规范](protocol.md)
- 说明：`lkit self install --flare-psk-file`（root-only 私密文件）在 daemon 启动前把
   psk 写入地盘 config.toml 的 `[flare]` 段（0600），daemon 首启即用该 psk 托管 flare；
   未提供时保留既有 `[flare]`，交互终端提示输入，无终端回落 daemon 自动生成。
   重复 `self install` 不改动已配置的 psk。

## FLR-21

**网络接管失败后经 flare 通道恢复（完整故障场景）**

- 测试层：未覆盖（缺口）
- 状态：`缺口`
- 说明：完整场景需要真实 L2 桥 + 容器内 systemd/网络服务破坏：网络接管失败使 IP
   路径不可用后，操作员经 `lflare` 隧道进入路由器执行 `lkit network rollback`。
   目前由 FLR-19（daemon 托管可连接）、FLR-20（daemon 部署供给 psk，早于接管）与
   接管 fixture e2e（[network.rs](../../lkit-cli/tests/install_fixture_e2e/network.rs)）
   分层覆盖，端到端联动留待后续专用容器脚本。
- 缺口：完整"接管失败 → lflare 连接 → rollback"的集成验证。

## FLR-22

**同一端口映射内多条 TCP 连接并发且数据相互隔离**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[连接生命周期脚本](../../scripts/flare/e2e-connections.sh)、
  [连接探针](../../scripts/flare/connection_probe.py)
- 说明：同一 `127.0.0.1:2222` 映射同时建立 32 条连接，每条发送不同的 128 KiB
  负载并校验回显，直接验证连接多路复用和四元组隔离。

## FLR-23

**正常关闭与 RST 短连接持续回收后新连接仍可用**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[连接生命周期脚本](../../scripts/flare/e2e-connections.sh)、
  [连接探针](../../scripts/flare/connection_probe.py)
- 说明：并发波次完成 480 条正常关闭连接和 192 条 `SO_LINGER=0` RST 连接；随后
  新建 16 条连接验证 EOF/错误路径已清理 smoltcp socket，未留下阻塞后续请求的状态。

## FLR-24

**慢读连接产生背压时其他连接仍可建立并完成传输**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[连接生命周期脚本](../../scripts/flare/e2e-connections.sh)、
  [连接探针](../../scripts/flare/connection_probe.py)
- 说明：12 条连接各发送 256 KiB 后延迟读取回显，使接收队列和 TCP 窗口形成背压；
  同时建立一条快速连接并断言其可独立完成，随后校验全部慢连接的数据完整性。

## FLR-25

**已建立 TCP 连接跨 stale 窗口空闲后继续传输**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[连接生命周期脚本](../../scripts/flare/e2e-connections.sh)、
  [连接探针](../../scripts/flare/connection_probe.py)
- 说明：连接建立并完成首次回显后保持 50 秒空闲，超过服务端 45 秒 stale 清扫窗口，
  再次在同一 TCP 连接上传输并校验数据，覆盖挂载/长连接空闲保活。

## FLR-26

**HTTP/1.1 持久连接并发请求通过同一映射**

- 测试层：flare e2e（Docker L2 bridge 双容器）
- 状态：`已覆盖`
- 证据：[连接生命周期脚本](../../scripts/flare/e2e-connections.sh)、
  [连接探针](../../scripts/flare/connection_probe.py)
- 说明：16 条并发 HTTP keep-alive 连接各连续发送 8 个具有独立路径的请求，共 128 次
  请求；逐次校验状态码和包含请求路径的响应体，覆盖 HTTP 端口映射的多连接复用。
