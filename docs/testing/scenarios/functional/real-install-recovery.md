# 真实用户安装与备份恢复

该验收在 Debian QEMU 虚拟机内使用生产 lkit 和固定版本的官方 Landscape，真实
systemd 管理服务。安装根位于第二块磁盘 `/srv/landscape`，lkit 地盘位于系统盘，
文件系统设备 ID 必须不同。版本和二进制、静态资产摘要由
[固定资产清单](../../../../scripts/qemu/network/landscape-release.json)确定。

API 请求遵循该版本发布的 OpenAPI：使用管理员登录取得令牌，通过正式 DNS 重定向和
DHCP API 修改数据库配置。断言比较配置实体语义和外部客户端观察结果，不比较 SQLite
字节，也不要求 API token、日志或指标历史恢复。生产程序不启用 `test-support`，
不注入假服务管理器或替身 Landscape。

该脚本只在 CI 中运行，不属于本地提交前检查。私有临时目录保存备份与配置快照；CI
归档仅包含公开版本摘要、断言结果和服务诊断，不上传 `.lkb`、管理员令牌或完整导出配置。
CI 的只读 GitHub token 通过虚拟机内 `0600` 的临时环境文件供 lkit CLI 和 daemon 查询官方
发布元数据，避免共享 runner 出口的匿名 API 限额；该文件随虚拟机销毁，不进入日志或归档。

## JNY-01

**正式安装后创建真实 DNS 与 DHCP 配置并备份**

- 测试层：真实 Debian/systemd QEMU、官方 Landscape、外部 LAN 客户端
- 状态：`部分覆盖`
- 证据：[QEMU 入口](../../../../scripts/test-qemu-network-takeover.sh)、[API 与数据断言](../../../../scripts/qemu/network/journey.py)、[虚拟机命令](../../../../scripts/qemu/network/journey-guest.sh)
- 说明：执行 `self install`、网络接管安装与显式确认；检查官方二进制和静态资产摘要、
  真实 SQLite 文件头、不同文件系统设备。通过真实登录创建两条 DNS 重定向，设置 DHCP
  地址池、租期和自定义选项，验证 API 导出包含这些实体。独立 LAN 客户端查询 DNS，
  验证指定地址实际生效。`backup create --output` 同时生成目录内和外部副本，CLI
  完整校验后检查两份字节一致、权限正确，归档包含官方二进制和真实导出配置。
- 缺口：同一业务链在 Alpine/OpenRC、Debian/SysVinit 的验收；实体范围尚未覆盖 NAT、
  DDNS、证书和全部配置类型，DHCP 下发效果尚未通过独立 DHCP 客户端验证。

## JNY-02

**仓库离线时从 ID 与外部备份恢复，并经客户端验证后确认**

- 测试层：真实 Debian/systemd QEMU、正式 API、外部 LAN DNS/HTTPS
- 状态：`部分覆盖`
- 证据：[离线与恢复编排](../../../../scripts/test-qemu-network-takeover.sh)、[配置变更与语义比较](../../../../scripts/qemu/network/journey.py)、[确认前状态断言](../../../../scripts/qemu/network/journey-guest.sh)
- 说明：备份后修改第一条 DNS 记录、删除第二条、增加新记录，并修改 DHCP 配置；
  通过 API、导出与真实 DNS 查询证明修改已经生效。QEMU 断开 WAN 虚拟网线，检查
  carrier 为零且官方仓库不可达，再使用备份 ID 恢复。恢复必须进入持久化确认阶段，
  此时安装 state 保持恢复前字节。外部客户端验证登录、前端页面、DNS 答案及导出配置
  等价恢复，新记录消失，之后执行 `network confirm` 并检查最终提交。
- 说明：删除原目录内备份、再次修改业务配置，使用仍保留的外部 `.lkb` 执行同样的离线
  恢复与确认。该流程验证外部副本独立可用，恢复不依赖仓库下载。
- 缺口：不同正式 Landscape 版本间恢复，以及真实业务恢复确认超时、拒绝确认和断电的
  完整故障矩阵。

## JNY-03

**已确认的恢复结果在整机重启后保持可用**

- 测试层：真实 QEMU 重启、systemd 自动启动、外部 LAN 客户端
- 状态：`部分覆盖`
- 证据：[整机重启编排](../../../../scripts/test-qemu-network-takeover.sh)、[重启后业务断言](../../../../scripts/qemu/network/journey.py)
- 说明：恢复确认后重新接通 WAN，重启整台虚拟机并检查 boot ID 改变；等待 lkit 和
  Landscape 自动启动。使用新登录令牌重新验证前端、DNS 答案和数据库导出的 DNS、DHCP
  与网络配置，确认没有未终结事务，安装 state 仍关联已提交的 restore 事务，WAN 保持
  计划中的 IPv4 配置。
- 缺口：OpenRC/SysVinit 上相同业务断言及非正常断电恢复。
