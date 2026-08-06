# 网络接管

## 使用边界

`lkit install --takeover-network` 是首次安装的显式破坏性模式，要求 root、真实可通信的
systemd 和交互终端。网卡始终由用户选择，lkit 不按默认路由或接口名自动决定 WAN/LAN。
无线、loopback 和虚拟接口不列入选择。已有 `br_lan`、不受支持的活动网络管理器
（`systemd-networkd`、wicked 或 connman），或 SELinux 已加载/配置为 enabled/permissive
时，在停止服务前失败。

接管支持 NetworkManager 和 Debian ifupdown 的 `networking.service`；不存在的 unit 保持
未安装状态且不会执行服务操作。接管不会卸载 NetworkManager、ifupdown、firewalld、
systemd-resolved 或其他软件包，也不收集 PPPoE 用户名、密码或 MTU。它保存这些宿主服务
的原始状态，然后依次 stop、disable、mask；回滚按原始 installed、enable 和 active 状态
恢复。

## 单网口

用户选择唯一 WAN 后必须确认 Landscape 不支持单臂 WAN/LAN 路由。lkit 从当前 SSH
服务端地址确定要保留的 IPv4/prefix，并要求该接口存在默认网关。初始化配置只创建：

- WAN 物理接口、原 IPv4/prefix 和默认网关；
- WAN route 与 Landscape firewall；
- TCP 22 和 6443 到 `Local` 的静态映射。

不创建 LAN bridge、LAN DHCP、PPPoE 或额外 WAN 地址。

## 多网口

用户先选择一个 WAN，再从剩余接口中选择一个或多个 LAN。lkit 创建 `br_lan` 并把所选
LAN 物理接口加入 bridge。管理地址默认 `192.168.10.1/24`，可在交互中修改；默认 DHCP
范围为子网内 `.100` 到最后一个可用地址。初始化配置只设置 WAN route、WAN firewall、
`br_lan` 的 LAN route 和 DHCP，不为 WAN 设置静态 IP、DHCP、PPPoE、NAT 或默认网关。
等待确认期间保留原网络管理器遗留的 WAN IPv4，作为尚未确认时的恢复入口；只有确认命令
通过新管理地址、接口和 Landscape 健康校验后，才清除该 WAN IPv4 并提交事务。

## 确认与回滚

停止宿主网络服务前，lkit 将自身复制为 root-only 恢复二进制，并安装三个事务专属 unit：

- 10 分钟确认期限的 persistent timer；
- timer 调用的幂等 rollback service；
- 未确认重启时在 Landscape 和 network-online 之前执行的 boot rollback service。

恢复机制 arm 成功后才停止 systemd-resolved、firewalld、`networking.service` 和
NetworkManager，其中 NetworkManager 在两者都存在时最后停止。Landscape 启动并通过健康
检查后，安装状态仍不提交。用户必须断开旧连接，重新执行 `ssh root@<管理地址>`，然后运行
`lkit network confirm`。确认会
检查 SSH 服务端地址、接口 MAC、管理 IPv4/prefix、bridge 成员、Landscape PID 和健康。
双网口模式还会在这些检查成功后清除 WAN 上继承的 IPv4；清除失败时不提交，恢复 timer
继续有效。

期限内未确认或确认前重启会清理未提交安装、恢复宿主服务状态并移除恢复 unit。恢复不
依赖原安装进程或原 SSH 连接存活。
