# Landscape Terrain 协议（L2 防失联通道）

Terrain 是 Landscape 路由器的 L2 旁路通信协议：当常规网络路径不可用时，主机通过
以太网帧与路由器建立加密的 TCP-over-IP 隧道（`lflare` 客户端 ↔ `lkit flare` 服务端），
用于应急管理连接。

协议名 Terrain，魔数 `TERR`；客户端可执行文件 `lflare`（服务端并入 `lkit flare`）。
共享密钥通过 `LANDSCAPE_FLARE_PSK` 环境变量或 `--psk` 提供。

## 帧格式

16 字节明文头 + 载荷：

| 字段 | 长度 | 说明 |
| --- | --- | --- |
| magic | 4 | `"TERR"`（`0x54455252`），未知帧在解析时丢弃 |
| version | 1 | `0x05`（v5：scrypt 主密钥 + 全阶段密钥派生） |
| type | 1 | 见下表 |
| session | 4 | 会话 ID（握手帧为 0 或协商值） |
| len | 2 | 精确载荷长度；以太网最小帧填充不算载荷 |
| seq | 4 | 会话内序号，用于 AEAD nonce 与重放防护；握手帧恒为 0 |

帧类型：`DISCOVER 0x01`、`RESP 0x02`、`AUTH_REQ 0x03`、`AUTH_ACK 0x04`、
`AUTH_NACK 0x05`、`KEEPALIVE 0x06`、`DATA 0x07`、`TEARDOWN 0x08`。

## 密钥计划

psk 从不直接使用：双方启动时用 scrypt 拉伸为 32 字节主密钥，离线攻击者每次 psk
猜测需付出约 32 MiB / ~100 ms（`N=2^15, r=8, p=1`；`LANDSCAPE_TERRAIN_SCRYPT_LOG_N`
可覆盖指数，夹取 10..=20，双方必须一致）。

所有派生以 `h(label, key, server_nonce, client_nonce) = sha256(label ‖ key ‖ s ‖ c)`
为基础：

| 密钥 | label | 输入 | 用途 |
| --- | --- | --- | --- |
| 主密钥 `master` | — | scrypt(psk) | 一切派生的根 |
| 发现前密钥 | `terrain-hkey0` | master, 0, 0 | 密封 DISCOVER；每帧自带随机 12 字节 nonce，固定密钥碰撞界 2^48 |
| 握手密钥 | `terrain-hsalt` / `terrain-hkey-c2s` / `terrain-hkey-s2c` | master, s_nonce, 0 | 密封 RESP/AUTH_REQ/AUTH_ACK/AUTH_NACK，每方向一钥 |
| 会话密钥 | `terrain-salt` / `terrain-key-c2s` / `terrain-key-s2c` | master, s_nonce, c_nonce | 密封 DATA/KEEPALIVE/TEARDOWN，每方向一钥 |
| 认证证明 | `terrain-auth-c2s` / `terrain-auth-s2c` | master, s_nonce, c_nonce | 握手挑战-响应 |

无前向保密：主密钥持有者可重派生任意会话密钥，对共享密钥的 LAN 协议可接受。

## 握手流程

1. **DISCOVER**（发现前密钥密封）：客户端广播设备名与可选发现令牌；只有 psk 持有者
   能被听到，名字与令牌永不明文上线。服务端对错误 psk 保持静默。
2. **RESP**：明文前缀携带服务端 nonce（用于派生握手密钥），其余密封，附设备名与
   允许转发的端口清单。
3. **AUTH_REQ**（握手密钥密封）：用户名 + 客户端证明。
4. **AUTH_ACK / AUTH_NACK**（握手密钥密封，含服务端证明）：认证失败原因只在密封
   帧中告知，防伪造 NACK 打断握手；已锁定源还会收到带剩余时长的锁定 NACK。
5. **会话**（会话密钥，ChaCha20-Poly1305）：AEAD nonce = 会话 salt(8) ‖ 帧序号(4)；
   16 字节明文头作为附加认证数据，篡改头部同样解密失败。`KEEPALIVE` 维持对端存活
   （45 秒 stale 清扫窗口内回显），`TEARDOWN` 优雅断开。

## 隧道与端口转发

- Linux 传输层使用 AF_PACKET 原始套接字（无 libpcap 运行时依赖）；Windows/macOS
  客户端使用 libpcap。服务端 Linux only。
- 每客户端会话内有一个 smoltcp 用户态 IP/TCP 栈（`127.0.0.1` 内部地址）；`lflare
  --forward 2222:6443` 在客户端监听 `127.0.0.1:2222`，隧道内连接到服务端
  `127.0.0.1:6443`。服务端仅允许转发白名单端口（默认 `22,6443`）。
- 重传、拥塞与滑动窗口由 smoltcp 在隧道内完成，丢包链路上数据仍保持完整。

## 服务端防护

- 每源 MAC 限速桶：DISCOVER/AUTH_REQ 默认 10/s，防扫描、爆破与踢会话。
- 全局限速桶（默认 200/s）：每帧伪造 MAC 可绕过单 MAC 限速，全局桶约束伪造洪泛的
  CPU 与 peer 表增长。
- 认证锁死：无活动会话的 MAC 60 秒窗口内 5 次失败即冻结 60 秒；会话失败解密另有
  每 MAC 预算，防止伪造帧洪泛消耗事件循环的解密算力。
- peer 表硬上限 4096，满后丢弃未知 MAC 的 DISCOVER。
- 活动会话永不被锁死：伪造者不能借受害者的 MAC 冻结其重认证。

## 部署形态

| 端 | 形态 | 入口 |
| --- | --- | --- |
| 客户端 | `lflare`（linux/windows） | `lflare --psk … --dev eth0 --forward 2222:6443` |
| 服务端 | `lkit flare serve`（Linux） | `lkit flare serve --psk … --dev any [--token …]` |
| 服务端（daemon 托管） | `lkit daemon` 读取 config.toml `[flare]` 段 | 段内配置 psk/device_name/ethertype/devices/forward_ports/token |
| 抓包诊断 | `lkit flare sniff` | 在线设备或 pcap 文件解码 Terrain 帧 |
