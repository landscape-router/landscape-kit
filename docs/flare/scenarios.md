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
