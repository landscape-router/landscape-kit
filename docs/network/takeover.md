# 网络接管

## 使用边界

`lkit install --takeover-network` 是首次安装的显式破坏性模式，要求 root、真实可通信的
systemd 和交互终端。网卡始终由用户选择，lkit 不按默认路由或接口名自动决定 WAN/LAN。
无线、loopback 和虚拟接口不列入选择。不受支持的活动网络管理器
（`systemd-networkd`、wicked 或 connman），或 SELinux 已加载/配置为 enabled/permissive
时，在停止服务前失败。已有 `br_lan` 不阻断安装：Landscape 按新配置接管或清理桥接现场，
lkit 不检查桥接是否存在。

接管支持 NetworkManager 和 Debian ifupdown 宿主，两种宿主的摘除方式不同：

- **NetworkManager、firewalld 与 systemd-resolved**：整体摘除。lkit 保存这些宿主服务的
  原始状态，然后依次 stop、disable、mask；不存在的 unit 保持未安装状态且不执行服务
  操作，回滚按原始 installed、enable 和 active 状态恢复。
- **ifupdown**：`networking.service` 不做整体停止。lkit 通过 `lkit-hostnet` 把选中接口
  （WAN + 全部选中 LAN）从 `/etc/network/interfaces`（含 `source`/`source-directory`
  引用的文件）细粒度摘除：stanza 的 method 改写为 `manual`、删除 `inherits` 与选项行，
  并从 `auto`/`allow-*` 等自动选择行中删除选中接口，避免 `networking.service` 或
  ifupdown hook 再次处理它们；未选接口逐字节保持原样、继续由宿主管理。原文件逐字备份
  到 lkit 地盘 `backups/hostnet`（同一主机只有一个 Landscape 安装，备份跨事务存活），
  回滚与卸载按 manifest 逐字恢复，恢复后按 `networking.service` 实况状态决定是否
  restart 重新套用原配置。选中接口不由 ifupdown 管理（配置文件缺失或不含其 stanza，
  如 NetworkManager 主机）时该步骤是 no-op。

摘除在停止宿主服务之前执行：保守解析拒绝或 dry-run 校验失败时宿主现场与安装现场均
未变动，安装直接中止。

接管、回滚与卸载都不会卸载 NetworkManager、ifupdown、firewalld、systemd-resolved 或
其他软件包，也不收集 PPPoE 用户名、密码或 MTU。

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

摘除宿主网络管理后，lkit 只对用户选中的 LAN 物理接口执行 IPv4 和 IPv6 address flush，
再启动 Landscape；未选择接口不执行地址清理，也不写入 Landscape 初始化配置。

等待确认期间由 Landscape 按计划维护 WAN 静态地址或 DHCP lease。只有确认命令通过接口、
管理地址和 Landscape 健康校验后，才提交事务。安装结束输出会向用户说明确认命令，并明确
提醒：未在确认期限内执行 `lkit network confirm`，安装将自动回滚。

## 确认与回滚

ifupdown 摘除在事务持久化为 Prepared 之后、安装恢复机制之前执行（见上文）。停止宿主
网络服务前，lkit 将自身复制为 root-only 恢复二进制，并安装三个事务专属 unit：

- 10 分钟确认期限的 persistent timer；
- timer 调用的幂等 rollback service；
- 未确认重启时在 Landscape 和 network-online 之前执行的 boot rollback service。

恢复机制 arm 成功后才停止 systemd-resolved、firewalld 和 NetworkManager，其中
NetworkManager 在两者都存在时最后停止；`networking.service` 不在停止之列（ifupdown
宿主靠配置摘除，服务保持原状态运行）。Landscape 启动并通过健康
检查后，安装状态仍不提交。用户可在任意可达主机的会话（推荐重新连接到管理地址，因为
摘除选中接口会断开其上的旧会话）运行 `lkit network confirm`。确认会
检查接口 MAC、管理 IPv4/prefix、bridge 成员、Landscape PID 和健康。

期限内未确认或确认前重启会清理未提交安装、恢复宿主服务状态并移除恢复 unit；ifupdown
摘除按 `backups/hostnet` 的 manifest 逐字恢复原文件，恢复成功且 `networking.service`
处于 active 时 restart 它重新套用原配置（重启失败只提示，不阻断其余恢复）。恢复不
依赖原安装进程或原 SSH 连接存活。
