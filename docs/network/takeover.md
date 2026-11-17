# 网络接管

## 使用边界

`lkit install --takeover-network` 是首次安装的显式破坏性模式，要求 root、真实可通信的
服务管理器（systemd、OpenRC 或 SysVinit）、启用且运行中的 lkit daemon 和交互终端。网卡始终由用户选择，lkit 不按默认路由或接口名自动决定 WAN/LAN。
无线、loopback 和虚拟接口不列入选择。不受支持的活动网络管理器
（wicked、connman 或独立 dhcpcd），或 SELinux 已加载/配置为 enabled/permissive
时，在停止服务前失败。SELinux 加载状态以内核 `enforce` 接口是否存在判定，空的
`/sys/fs/selinux` 挂载点不表示已加载。已有 `br_lan` 不阻断安装：Landscape 按新配置接管或清理桥接现场，
lkit 不检查桥接是否存在。

网络发现和部署运行时通过 PATH 调用完整 iproute2 的 `ip`，不假定它安装在
`/usr/sbin/ip`。网络向导与后台部署使用同一命令选择；显式测试运行时仍可覆盖命令路径。
调用失败时诊断包含命令和参数，而不只显示缺少文件的 I/O 错误。
三个后端的控制台安装均进入网络向导。网络接管的恢复契约见
[daemon 网络接管恢复](daemon-recovery.md)。

systemd 支持 systemd-networkd、NetworkManager、Debian ifupdown、firewalld 和 systemd-resolved；
OpenRC/SysVinit 支持 `networkmanager`／`NetworkManager`、`networking` 和 `firewalld`
服务脚本。预检查、接管发现与连接快照共用 NetworkManager 服务别名；按实际存在的
脚本名称分别保存、停止与恢复，不把 Alpine 的小写名称改写成大写。
不存在的服务不操作。服务状态无法可靠查询时在接管前失败。systemd 使用
整组先 mask 再 stop，不调用 disable/enable；回滚只移除对应 mask 并保留原启动链接；OpenRC/SysVinit 停止服务并保存、移除其开机链接，回滚恢复原始
runlevel、链接顺序及运行状态。接管不卸载软件包，不收集 PPPoE 凭据或 MTU。

初始化配置的生成方式按目标 release 版本区分:≥ 0.25.1 时 lkit 调用目标 release 目录下
`landscape-webserver config` 子命令生成(Landscape 的稳定部署接口,生成文件内嵌目标
二进制版本、只能被同版本导入);更早版本由 lkit 进程内手拼。两条路径产出下述相同的
语义内容,详见[网络重配置](reinit.md)的「新配置生成」一节。

## 单网口

用户选择唯一 WAN 后必须确认 Landscape 不支持单臂 WAN/LAN 路由。CLI 优先使用该接口
发现顺序中的第一个 IPv4/prefix 和第一个默认网关生成静态 WAN；任一静态信息缺失时生成
DHCP WAN。初始化配置只创建：

- WAN 物理接口，以及静态 IPv4/prefix/网关或 DHCP client；
- WAN route 与 Landscape firewall；
- 静态 WAN 下 TCP 22 和 6443 到 `Local` 的静态映射。

不创建 LAN bridge、LAN DHCP、PPPoE 或额外 WAN 地址。

## 多网口

用户先选择一个 WAN，再从剩余接口中选择零个或多个 LAN。选择一个或多个 LAN 时，lkit
在初始化配置中声明 `br_lan`，由 Landscape 创建桥接并把所选 LAN 物理接口加入 bridge。
管理地址默认 `192.168.10.1/24`，可在
交互中修改；默认 DHCP 范围为子网内 `.100` 到最后一个可用地址。未选择 LAN 时按 WAN-only
模式处理，不创建 `br_lan`，也不启用 LAN DHCP；其他未选物理接口不进入 Landscape 配置。

WAN-only 模式初始化配置与单网口模式相同：静态模式设置 WAN IPv4、默认网关、WAN route、
Landscape firewall 和管理端口的 Local 静态映射；DHCP 模式设置 WAN DHCP client、route
与 firewall。RoutedLan 模式同样显式写入 WAN 的静态或 DHCP 配置，并设置 `br_lan` 的 LAN
route 和 DHCP。初始化配置中只有 `br_lan` 的 zone type 为 `lan`，所选 LAN 物理接口的
zone type 为 `undefined`，只通过 controller（上游）关联到 `br_lan`。CLI 使用所选 WAN
发现顺序中的首个 IPv4 和该接口首个默认网关；缺任一项时
使用 DHCP。控制台向导展示相同的 IPv4 与网关：WAN 配置面板顶部用静态 / DHCP 客户端
两个 tab 切换模式，选中 WAN 后预填两项（完整对默认 Static，缺任一项默认 DHCP），静态
模式下地址/CIDR 与网关在同一页编辑，页面底部是“确认并继续”按钮；LAN 的 DHCP 配置
（管理地址、地址池起始与结束）在同一页表单中一次性填写并确认。向导结束前显示完整计划
摘要并要求用户确认；摘要明确所选 LAN 会清理 IPv4/IPv6 地址，未选择接口不会接管或修改。
网络计划中的接口列表只包含 WAN 和用户选中的 LAN，不自动接管其他物理接口。

停止宿主网络服务后，lkit 只对用户选中的 LAN 物理接口执行 IPv4 和 IPv6 address flush，
再启动 Landscape；未选择接口不执行地址清理，也不写入 Landscape 初始化配置。

等待确认期间由 Landscape 按计划维护 WAN 静态地址或 DHCP lease。只有确认命令通过接口、
管理地址和 Landscape 健康校验后，才提交事务。安装结束输出会向用户说明确认命令，并明确
提醒：未在确认期限内执行 `lkit network confirm`，安装将自动回滚。

## 确认与回滚

停止宿主网络服务前，lkit 持久化服务状态、NetworkManager 活动连接快照、确认期限与主机启动标识，并检查 daemon
具备恢复能力。确认窗口为 10 分钟，由 daemon 监测；新事务不创建专属恢复 unit。

Landscape 启动并通过健康检查后安装状态仍不提交。用户运行 `lkit network confirm`
完成接口与健康复检后提交事务。期限内未确认或确认前主机重启触发自动回滚；daemon
自身重启继续原期限。回滚失败保留 `rolling_back` 并重试。已接受确认的 `finalizing`
继续提交。旧事务的 systemd 恢复 unit 在处理完成时清理。

## systemd-networkd 接管

networkd 主服务、netlink/varlink 等配套 socket、wait-online 服务及模板构成接管组，
模板使用 unit 文件状态查询，不作为运行实例启停；已加载的 wait-online 实例逐个保存，支持 systemd 的 `\xHH` 转义实例名。
不存在的单元跳过。整组运行状态和启用状态先写入事务，再将整组 mask，最后停止。
接管保留启动链接和 `.network`/`.netdev` 配置，不调用 networkd 的 disable/enable，
避免 `Also=` 连带改变 socket、网络生成器和 wait-online 的启用配置。

回滚仅删除指定单元指向 `/dev/null` 的 mask 并执行 daemon-reload，不调用会连带
清理链接的 `systemctl unmask`。mask 被外部改成其他链接或文件时拒绝覆盖，保留事务。
状态保存时检查 mask 路径；同名本地 unit 文件或非 mask 链接会在停止服务前被拒绝，
避免覆盖自定义 unit。
先解除整组屏蔽，再恢复原本运行的单元，随后停止原本不运行的单元，并还原
原有 persistent/runtime mask。只有 mask、没有底层 unit 文件的单元也恢复原始屏蔽，
已停止或不存在的运行实例不执行 stop。恢复可重复执行；任何失败保留事务供 daemon 重试。
确认后整组保持停用和屏蔽。此支持限定 systemd 后端；OpenRC/SysVinit 不自动接管
名为 systemd-networkd 的自定义服务脚本。

## NetworkManager 连接恢复

活动的 NetworkManager 必须提供可用的 `nmcli`。接管前保存非 loopback 活动连接的
UUID、接口、物理接口永久 MAC，以及当时具备的 IP 地址族和默认网关。回滚按 UUID 显式
激活原连接，并检查绑定接口、地址族和默认网关；DHCP 租约地址允许变化。接口被替换、
连接被删除或连接尚未恢复时，事务保持 `rolling_back`，daemon 重试，不宣告回滚完成。
只恢复服务进程的 active 状态不能代替连接校验。Flare 所需的网卡拉起保留，
NetworkManager 不依赖对提前收到 IPv6 RA 的接口自动选对连接。
systemd 的 `NetworkManager.service` 与 OpenRC/SysVinit 的两种脚本名称使用相同的
连接保存和恢复规则；任何已发现的活动别名都要求连接快照。

旧事务缺少新快照标识时按原启用状态恢复服务；未记录的自定义启动链接无法重建。
缺少连接快照时输出需要人工核验连接的提示；
不推断或激活无法证明属于原部署的连接。
