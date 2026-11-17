# QEMU 服务管理后端验收

真实系统测试按发行版和 init 后端分别运行。Debian trixie/systemd、Alpine 3.22.2/OpenRC
与 Debian trixie/SysVinit 都使用 x86_64 KVM 虚拟机，但测试范围不同。daemon 的恢复决策
由 lkit 实现；服务注册、开机启用、停止和进程托管仍需要对应后端的真实系统验证。

## 当前矩阵

| 发行版与后端 | 入口 | 直接验证的行为 | 验证边界 |
| --- | --- | --- | --- |
| Debian trixie / systemd | [QEMU 网络接管](../../scripts/test-qemu-network-takeover.sh) | 正式 provider 下载真实 Landscape；NetworkManager、firewalld、resolved 网络接管；新 LAN SSH 确认；未确认重启回滚；恢复失败重试及原连接激活 | 仅 NetworkManager 管理真实网卡；未覆盖 networkd 的真实网卡接管、确认超时、提交后重启或断电 |
| Alpine 3.22.2 / OpenRC | [Alpine QEMU](../../scripts/test-qemu-alpine.sh) | 生产 musl daemon 注册、重复安装、崩溃拉起、真实重启、bpffs 及 cgroup、真实 L2 恢复通道、OpenRC 启动顺序、自移除和 Docker 服务 | Landscape 是最小进程 fixture；没有执行真实 Landscape 网络接管；没有禁用 daemon 后重启保持停机的直接断言 |
| Debian trixie / SysVinit | [SysVinit QEMU](../../scripts/test-qemu-sysvinit.sh) | 真实 PID 1 与后端识别；daemon 注册、启停、崩溃拉起；启用和禁用后的真实重启；自移除 | daemon 生命周期基线；没有部署 Landscape 或执行网络接管；不能外推为 Devuan 等其他发行版验收 |

systemd 的补充契约由 [systemd-nspawn](nspawn-systemd.md) 验证，包括 daemon 原生操作、
前端断开后继续执行、真实 manager 下的崩溃恢复，以及 networkd 的服务、socket、mask 和
启动链接保存恢复。networkd 场景的网络发现与 Landscape 使用 fixture，不提供真实网卡
接管后 SSH 可达的证据。

详细断言见 [systemd 场景](scenarios/systemd-smoke.md)、
[Alpine 场景](scenarios/functional/alpine.md)和
[SysVinit 场景](scenarios/functional/sysvinit.md)。场景的覆盖状态表示脚本包含直接断言；
某次提交是否通过，必须按该提交的 Actions 运行结果判定。

## 逐个后端确认

每次跨后端验收应使用同一提交的生产构建，并分别记录以下三个 workflow 的 commit SHA、
run ID、结论及诊断产物：

- [QEMU network takeover](../../.github/workflows/test-qemu-network-takeover.yml)。
- [Alpine apk, bpffs and OpenRC](../../.github/workflows/test-qemu-alpine.yml)。
- [QEMU SysVinit lifecycle](../../.github/workflows/test-qemu-sysvinit.yml)。

启动测试不等于通过验收；应等待每个 workflow 完成，核对 guest 断言和真实重启后的结果。
systemd 的 `<scenario>-environment.log`、OpenRC 的 `initial.log` 和 SysVinit 的三个阶段
日志保存实际发行版、内核、PID 1 可执行文件与 lkit 版本，并断言 lkit 检测的后端。
脚本把 boot ID 改变作为真实重启的证据，不能用 daemon 重启或 fixture 修改 boot ID 替代。
失败时先查看 Actions 失败步骤，再核对串口、guest 阶段日志、进程与服务状态；修复后用
新提交重新运行受影响的后端。更旧提交的成功结果不能代替当前提交。

daemon 的崩溃验收区分监管进程和 daemon 子进程：强杀 daemon 后，监管进程 PID 保持
不变，daemon PID 改变且服务恢复。主动停止则应停止整个服务，不再拉起。1s、3s、5s、10s
封顶且持续重试、稳定运行后重置的精确时间策略由单元和进程 fixture 验证；一次 VM
崩溃拉起不能证明整个退避序列。对应 [DAE-13](scenarios/functional/daemon.md#dae-13)。

## 执行环境与隔离

三个 QEMU 入口都要求 Linux x86_64、root 和可用的 `/dev/kvm`，不回退到 TCG。
Alpine 和 SysVinit 入口还要求显式 `LKIT_E2E=1`。它们属于 CI 系统测试，不属于提交前
单元测试；本地专项实测应使用专用测试主机或与工作环境隔离的运行环境。

Alpine 和 SysVinit 的管理 SSH 绑定宿主随机回环端口，QEMU user networking 使用
`restrict=on`，guest 不访问外网，不创建宿主 TAP、桥或 bpffs 挂载。Alpine 的真实 L2
通信只在 guest 内部的 namespace/veth 中进行。rootfs 构建阶段需要访问发行版签名仓库；
guest 的离线运行不代表构建过程离线。

Debian/systemd 网络接管需要双网卡和宿主 TAP，配置 `192.168.10.2/24`，并从正式 provider
下载 Landscape。执行隔离及预编译入口见 [QEMU 网络接管](qemu-network-takeover.md)。
使用容器运行时，不共享宿主网络或 PID 命名空间；仓库和预编译二进制只读挂载，日志单独
可写。Alpine 的 rootfs 构建使用 Docker，所需构建环境由其 CI workflow 准备。

所有专项运行应显式指定 `LKIT_QEMU_ARTIFACT_DIR` 保存日志。网络接管入口的默认日志位于
退出时删除的临时目录；Alpine/SysVinit 日志与串口、guest 诊断由对应 workflow 归档。

## 完整网络接管的验收边界

服务能启动与网络接管能恢复是不同的验收范围。OpenRC/SysVinit 生命周期通过后，完整
网络接管仍需要在对应后端的真实网络服务上直接验证：

| 场景 | 必须观察的结果 |
| --- | --- |
| 接管前保存 | 原服务运行状态、启用状态、配置和启动链接均被保存；OpenRC 保留多个 runlevel，SysVinit 保留 S/K 链接与顺序，systemd 保留 enable/mask 和自定义链接 |
| 待确认时重启 | boot ID 改变；Landscape 启动检查阻止使用待确认配置；daemon 恢复原 IPv4、默认路由和远程 SSH；事务完成回滚 |
| 同次开机确认超时 | 到达期限后回滚，宿主网络和 SSH 恢复；不依赖用户重新打开 CLI/TUI |
| 宿主恢复暂时失败 | 事务保留回滚中和恢复凭据；恢复条件满足后 daemon 重试成功，不提前清理状态 |
| 用户确认 | 在新 LAN 地址执行确认；只有健康检查通过且仍在期限内才提交；WAN 地址符合最终计划 |
| 确认后重启 | 已提交配置启动成功；接管前的网络管理服务不会重新干预；远程管理仍可达 |
| 原本停止或禁用的服务 | 回滚保持原来的运行与启用设置，不把原本停机的服务自动启动 |

当前 Debian/systemd QEMU 直接覆盖其中的 NetworkManager 接管、确认、未确认重启及
恢复失败重试。OpenRC/SysVinit 的完整网络接管、其他发行版、其他架构与不同网络栈仍需
分别建立真实系统证据，不能由后端单元测试或 daemon 生命周期基线推断已通过。
