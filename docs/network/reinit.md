# 网络重配置(reinit)

`lkit reinit` 对已接管宿主网络的 systemd 安装重新收集 WAN/LAN 计划并重建配置。接口
发现、交互选择与确认机制复用[网络接管](takeover.md),差异只在本页描述。

## 使用边界

- 只接受已提交、`service.manager == systemd` 且宿主网络已接管的安装（systemd-resolved
  被 stop/disable/mask，或存在未恢复的 hostnet 摘除备份——ifupdown/NM/firewalld 任一
  适配器）;未接管时返回参数错误,reinit 不负责首次接管;
- 网卡始终由用户重新选择,lkit 不按默认路由或接口名自动决定 WAN/LAN;无线、loopback
  和虚拟接口不列入选择。接口集合必须与首次接管的摘除现场一致(角色、
  地址与 DHCP 可以变,换接口不行):lkit 按地盘 hostnet 备份反查各适配器的摘除现场
  (ifupdown manual stanza、NM drop-in 条目、firewalld 快照差集)取并集并与新选择
  比对,不一致时在创建事务前拒绝,提示先 `lkit uninstall` 恢复宿主配置,再重新
  `lkit install --takeover-network`;
- reinit 不停止、不重新 disable/mask 宿主网络服务(它们维持已接管状态),也不重放
  接口摘除(备份与改写现场保持首次接管的状态),但仍会 arm
  恢复二进制、确认 timer 与 boot rollback(确认窗口与首次接管一致);
- 与首次接管相同,reinit 不收集 PPPoE 用户名、密码或 MTU,不卸载任何软件包。

## 网络计划收集

与 `lkit install --takeover-network` 完全一致:

- CLI 通过 `discover` + 交互提示选择 WAN,再从剩余接口选择零个或多个 LAN;未选择 LAN
  时按 WAN-only 处理;
- 静态信息完整时使用所选 WAN 发现顺序中的首个 IPv4/prefix 与首个默认网关生成静态
  WAN,任一缺失时生成 DHCP WAN;多网口默认管理地址 `192.168.10.1/24`,默认 DHCP 范围
  为子网内 `.100` 到最后一个可用地址,均可交互修改;
- 向导结束前显示完整计划摘要并要求确认;控制台复用相同的 NetworkWizard。

## 新配置生成

新 `landscape_init.toml` 的生成方式按活动版本分两条路径:

- **活动版本 ≥ 0.25.1**:调用活动 release 目录下 `landscape-webserver` 二进制的
  `config` 子命令(`--stdout`)生成。这是 Landscape 侧文档化的稳定部署接口:lkit 只依赖
  其 flag 面,不再镜像 `landscape_init.toml` 的内部格式;生成的文件内嵌目标二进制版本,
  只能被同版本导入,因此必须由目标 binary 自己生成。lkit 传入的参数:admin 凭据
  (`--admin-user`/`--admin-pass`)、WAN/LAN 拓扑与地址、DHCP 范围与租期(43200 秒,与旧
  路径一致)、WAN 管理静态映射(`--static-nat 22:22 --static-nat 6443:6443`),以及
  `--enable firewall --disable nat`(lkit 的服务集一直是 WAN 上 firewall 而非 nat);
- **活动版本 < 0.25.1 或无网络计划的安装**:维持 lkit 进程内的 `LandscapeInit`
  手拼路径(旧版本不具备完整 flag 面,该路径冻结保留,仅服务旧版本)。

两条路径产出的语义一致,`version` 固定为当前活动版本:

- 凭据:用户重新输入的 admin 用户与密码;
- 网络实体:WAN 物理接口与静态 IPv4/prefix/网关或 DHCP client、WAN route、Landscape
  firewall,静态 WAN 下 TCP 22 和 6443 到 `Local` 的静态映射;RoutedLan 额外创建
  `br_lan`、LAN DHCP 与 LAN route;只有 `br_lan` 的 zone type 为 `lan`,所选 LAN 物理
  接口的 zone type 为 `undefined`,仅通过 controller(上游)关联到 `br_lan`;
- 除以上实体外的全部配置清空,由 Landscape 重建数据库;自签名证书等派生资产由
  Landscape 重新生成。

## `br_lan` 与地址清理

首次接管时 `br_lan` 由 Landscape 创建。reinit 不检查、不协调内核中的 `br_lan` 桥接
现场——无论新计划是否包含 LAN，Landscape 都会在初始化时按新配置创建桥接、同步成员
或清理不再管理的桥接，lkit 不做任何 `br_lan` 存在性判断或成员操作。install 的
`--takeover-network` 同样不检查桥接是否存在。

reinit 只对新计划中选中的 LAN 物理接口执行 IPv4/IPv6 address flush(与首次接管的
`clear_selected_lan_addresses` 规则一致);WAN 接口不执行地址清理,地址由 Landscape
按新配置维护(静态写入或 DHCP lease);未选择接口不执行任何修改。

## 确认与回滚

管理地址可能变化导致 SSH 断线,因此 v1 一律进入确认窗口(无论地址是否变化):

- 健康检查通过后 arm 恢复机制并提交 pending 状态,进入 `awaiting_network_confirmation`;
- `lkit network status` 显示事务阶段、新管理地址与确认截止时间;
- `lkit network confirm` 复核接口 MAC、管理 IPv4/prefix、`br_lan` 成员、Landscape
  PID 与健康后提交 state 并移除恢复 unit;
- 10 分钟 timer 到期、确认前重启与手工 `lkit network rollback` 都进入同一幂等回滚:
  停止 Landscape → 恢复事务目录中的旧 `data/` → 重启旧配置并健康检查 → 标记
  `rolled_back`;桥接现场随旧数据恢复,由 Landscape 按旧配置重新接管;
- 确认与回滚入口接受 `Operation::Reinit` 事务,判定规则与首次接管相同(只接受
  `awaiting_network_confirmation`、`finalizing`、`rolling_back` 阶段)。

## 边界

- install 与 reinit 都不检查 `br_lan` 是否存在,桥接的创建、成员同步与清理全部由
  Landscape 按新配置处理;
- 待确认的 reinit 事务与首次接管一样阻断其他命令进入(含再次 reinit);
- 回滚成功后的安装保持可用,现场与保护 `.lkb` 保留;用户可用 `lkit restore` 从保护
  备份人工恢复被清空的非网络配置。
