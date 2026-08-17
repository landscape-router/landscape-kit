# `check`：Landscape 部署前检查

## 目标

`lkit check` 用于在部署 Landscape Router 之前检查当前主机是否满足运行条件，并尽早发现会导致 Landscape 无法启动或网络功能异常的问题。

首版只检查**当前本机环境**，不检查待部署文件。Landscape 主体、静态页面、geoip/geosite 数据和初始化配置文件都将从远端下载，因此这些内容不属于本命令的检查范围。

检查逻辑应设计为可复用的函数，未来由 `install` 命令调用。CLI 只负责调用检查、格式化报告和设置退出码，检查函数本身不得直接打印终端文本或退出进程。

## 适用范围

- 支持使用 glibc 的 Linux 主机；当前发布产物不支持 Alpine 等 musl 发行版。
- 内核版本要求为 `6.9` 或更高版本。
- 命令必须以 `root` 身份运行。
- 检查过程只读，不修改主机状态。
- 不停止、启用、禁用或重启任何系统服务。
- 不修改 `/etc/resolv.conf`、sysctl、网络接口、路由、防火墙或 SELinux 配置。
- 不通过短暂占用端口的方式进行探测。

非 Linux 或非 root 环境应报告错误，并仍尽可能输出已经能够完成的检查结果。发行版名称
只用于诊断，不作为部署门槛；依赖安装建议根据主机上实际可用的包管理器选择。

## 结果模型

检查函数返回结构化结果，至少包含：

- 检查项稳定标识符，例如 `platform.linux`、`kernel.version`、`port.dns`。
- 检查项标题。
- 检查状态：`pass`、`warning`、`error` 或 `unknown`。
- 当前检测到的值或状态。
- 原因说明。
- 建议的人工处理动作。
- 可选的详细信息，例如进程名、PID、监听地址或配置来源。

同时返回汇总状态：

- `pass`：检查已完成且条件满足，可以继续部署。
- `warning`：检查已完成，但存在非阻断风险，可以继续部署。
- `error`：检查已完成且确认存在部署阻断项，不应继续部署。
- `unknown`：由于权限、工具或系统接口不可用，无法完成检查，不能宣称环境满足要求。

状态聚合优先级为 `error` > `unknown` > `warning` > `pass`。CLI 输出使用人类可读的分组报告；存在 `error` 或 `unknown` 时返回非零退出码，只有 `warning` 时返回零退出码。未来的 `install` 默认也应在 `error` 或 `unknown` 时停止后续安装步骤。

检查工具缺失、权限不足或系统接口不可读取时，使用 `unknown`，并在结果中说明具体缺失项和人工处理建议；只有已经确认环境不满足要求时才使用 `error`。

## 检查项目

### 1. 运行身份与平台

| 标识符 | 检查内容 | 不满足时 |
| --- | --- | --- |
| `runtime.root` | 当前有效用户 ID 是否为 `0` | `error` |
| `platform.linux` | 当前系统是否为 Linux | `error` |
| `platform.distribution` | 记录 `/etc/os-release` 的发行版 ID，供诊断使用 | `pass` 或 `warning` |
| `platform.architecture` | 记录当前 CPU 架构，供后续安装包选择和诊断使用 | `pass` 或 `warning` |

首版已知发布产物优先支持 `x86_64` 和 `aarch64`。检测到这两个架构时报告 `pass`；其他架构不直接判定为无法运行，但报告 `warning`，提示安装包或 eBPF 产物可能不可用。无法读取架构时报告 `unknown`。

发行版及其版本不使用白名单。兼容性由 Linux、架构、内核版本和后续运行能力检查决定；
`/etc/os-release` 无法读取时只报告 `warning`。发布安装脚本在能够识别 musl 时提前拒绝，
避免安装当前 glibc 动态链接产物后才执行失败。

### 2. 内核版本与配置

#### 内核版本

检查运行中的内核版本，要求满足 `>= 6.9`。版本低于要求时报告 `error`，并显示当前版本和要求版本。

#### BPF、BTF 和 Cgroup

检查项应覆盖 Landscape eBPF 运行所需的核心能力：

| 标识符 | 检查内容 | 不满足时 |
| --- | --- | --- |
| `kernel.bpf` | 通过只读 `bpf()` 系统调用探测 BPF 子系统，并检查 BPF JIT 状态 | `error` 或 `unknown` |
| `kernel.btf` | 内核 BTF 信息存在且可读取，优先检查 `/sys/kernel/btf/vmlinux` | `error` 或 `unknown` |
| `kernel.cgroup` | Cgroup 文件系统已挂载且可用 | `error` |
| `kernel.cgroup_cpu` | Cgroup CPU controller 可用 | `error` |
| `kernel.cgroup_bpf` | Cgroup BPF 支持已启用 | `error` |
| `kernel.bpf_events` | BPF events 支持已启用 | `error` |

`kernel.bpf` 的系统调用探测必须使用不会创建或修改 BPF 对象的操作，例如 `BPF_PROG_GET_NEXT_ID`：

- 返回成功或 `ENOENT`：说明 syscall 和 BPF 子系统可用，继续检查 JIT；
- 返回 `ENOSYS`：说明内核不支持 BPF syscall，报告 `error`；
- 返回 `EPERM`/`EACCES`：说明当前权限不足，root 场景报告 `error`，其他场景报告 `unknown`；
- 其他错误：报告 `unknown`，保留 errno 和错误文本，不将其误判为支持或不支持。

BPF JIT 使用只读方式检查 `/proc/sys/net/core/bpf_jit_enable`：值为 `1` 或 `2` 时通过，值为 `0` 时报告 `error`；文件不存在或不可读时，结合 `CONFIG_BPF_JIT` 配置判断，配置和运行状态都无法确认时报告 `unknown`。

内核配置读取应按可用性尝试以下来源：

1. `/proc/config.gz`；
2. `/boot/config-<running-kernel>`；
3. `/lib/modules/<running-kernel>/config`。

配置文件不可读取时，不能简单地将所有配置判定为关闭；相关配置检查应报告 `unknown`，并给出对应的诊断信息。BTF 文件实际不存在时报告 `error`；BTF 文件存在但不可读取时报告 `unknown`，除非已经确认内核接口明确返回不支持。

如果配置项为模块（`m`）而不是内置（`y`），应根据该能力是否已实际加载判断；不能只根据配置文本判断通过。

### 3. 资源限制

| 标识符 | 检查内容 | 不满足时 |
| --- | --- | --- |
| `resource.memory` | 主机内存是否至少为普通发行版建议的 `2 GiB` | `error` |

检查结果应同时记录总内存和可用内存。内存不足是部署阻断项，不应仅作为提示继续安装。

`RLIMIT_MEMLOCK` 由 install 通过 systemd unit 的 `LimitMEMLOCK=infinity` 保证，check 不再检查当前进程的 memlock 限制；install 必须确保该配置存在，否则 Landscape 的 eBPF 加载可能失败。

### 4. 必需命令与运行时依赖

以下依赖属于首版硬性要求：

| 标识符 | 检查内容 | 不满足时 |
| --- | --- | --- |
| `dependency.iproute2` | `ip` 命令存在且可执行 | `error` |
| `dependency.tc` | `tc` 命令存在且可执行，并支持 BPF 相关功能 | `error` 或 `unknown` |
| `dependency.pppd` | `pppd` 命令存在且可执行，用于 PPPoE 拨号 | `error` |

报告中应显示缺失的具体命令和适用于当前包管理器的安装建议，而不是只显示“依赖缺失”。
常见映射如下：

| 包管理器 | `ip` / `tc` | `pppd` |
| --- | --- | --- |
| Debian/Ubuntu `apt` | `iproute2` | `ppp` |
| Fedora/RHEL `dnf` 或 `yum` | `iproute` | `ppp` |
| Arch Linux `pacman` | `iproute2` | `ppp` |
| openSUSE `zypper` | `iproute2` | `ppp` |

尤其应明确 `pppd` 是命令名，软件包名通常是 `ppp`。无法识别包管理器时，提示用户安装
提供相应命令的软件包，不猜测一条可能不可用的安装命令。

`tc` 的 BPF 能力检查使用只读命令 `tc filter help`：命令必须成功退出，且合并后的标准输出和标准错误中（大小写不敏感）包含 `bpf`。`tc` 命令不存在时报告 `error`，因为 `iproute2` 是硬性依赖；命令存在但执行失败或输出无法读取时报告 `unknown`；命令成功但帮助文本不包含 `bpf` 时报告 `error`。不执行 `tc filter add` 等会改变网络状态的命令。

Docker/Podman 属于软性依赖：

| 标识符 | 检查内容 | 不满足时 |
| --- | --- | --- |
| `dependency.container_runtime` | `docker` 或 `podman` 至少一个可用 | `warning` |

缺少容器运行时不阻断基础部署，但必须说明：需要将流量分流到容器时必须安装并配置 Docker 或 Podman。

### 5. 端口冲突

使用只读方式检查本机监听套接字，优先使用系统接口或等效方式；不得通过 bind/close 方式抢占端口。发现监听者时尽可能解析：协议、监听地址、端口、进程名和 PID。

| 标识符 | 默认端口 | 检查内容 | 不满足时 |
| --- | ---: | --- | --- |
| `port.dns` | `53` | TCP/UDP DNS 端口是否已被其他服务占用 | `error` |
| `port.http` | `6300` | Landscape HTTP 管理端口是否已被占用 | `error` |
| `port.https` | `6443` | Landscape HTTPS 管理端口是否已被占用 | `error` |

DNS `53` 端口是硬性冲突，Landscape DNS 服务无法在端口被占用时启动。HTTP/HTTPS 管理端口同样属于启动所需端口；如果未来支持自定义端口，应由调用方传入待检查端口，而不是把 `6300/6443` 永久写死在检查层。

无法识别占用进程但确认端口监听时，仍报告错误，并在详细信息中说明“监听者信息不可读取”。

### 6. 系统服务与安全策略

这些项目不修改系统，只检查当前状态并提供处理建议。

| 标识符 | 检查内容 | 结果规则 |
| --- | --- | --- |
| `service.network_manager` | `NetworkManager` 是否安装、运行或开机启用 | 运行中为 `error`；仅安装/启用为 `warning` |
| `service.systemd_resolved` | `systemd-resolved` 是否运行或开机启用 | 运行中且占用 DNS 端口时由 `port.dns` 报 `error`；其他情况为 `warning` |
| `service.firewalld` | `firewalld` 是否运行或开机启用 | 运行中或开机启用为 `error`，提示其可能阻断 Landscape 网络规则 |
| `security.selinux` | SELinux 是否启用及是否为 enforcing | enforcing 为 `warning`，提示需要额外放行 Landscape 权限 |

服务检查需要区分“未安装、已安装未运行、正在运行、已启用”，避免把所有状态都混为一个结果。

### 7. DNS 配置风险

| 标识符 | 检查内容 | 结果规则 |
| --- | --- | --- |
| `dns.resolv_conf` | 检查 `/etc/resolv.conf` 是否存在、是否为符号链接以及当前 nameserver 内容 | 正常为 `pass`；存在可恢复性风险时 `warning` |

必须明确提示：Landscape 启动 DNS 服务时可能把 `/etc/resolv.conf` 指向 `127.0.0.1`；停止 Landscape 后如果主机无法解析域名，应优先检查该文件。本项只读，不自动备份或修改文件。

### 8. lkit 常驻服务

| 标识符 | 检查内容 | 结果规则 |
| --- | --- | --- |
| `service.lkit_daemon` | 全局常驻 daemon（`lkit daemon`）是否运行中（读取地盘 pidfile，进程存活即运行中） | root 且未运行为 `error`；非 root 未运行为 `warning`；运行为 `pass` |

root 会话的安装与生命周期命令（install/switch/update/repair/restore/reinit/uninstall 等）都委托
给常驻 daemon 执行，daemon 未运行时这些命令必然失败；该检查在部署前就暴露问题，
建议动作是 `lkit self install`（注册并启动 daemon）。非 root 会话内联执行命令，不要求
daemon，只报告 `warning`。控制台 Install 面板的部署前检查复用本检查：root 下 daemon
未运行时，检查汇总为阻断状态，未部署 daemon 前无法进入安装表单。

## 输出顺序

CLI 按以下顺序输出，保证人工阅读和未来日志解析稳定：

1. 运行身份与平台；
2. 内核版本与内核能力；
3. 资源限制；
4. 必需命令与软性依赖；
5. 端口冲突；
6. 系统服务与安全策略；
7. DNS 配置风险；
8. lkit 常驻服务；
9. 汇总结果和继续部署建议。

每个项目只输出一条主结果；详细的命令输出、配置来源和占用者信息放入该项目的 `details` 字段或 verbose 输出，避免默认报告过长。

## 非目标

- 不下载或校验 Landscape 发布文件。
- 不检查静态页面是否存在或内容是否完整。
- 不解析或验证 `landscape_init.toml`。
- 不检查具体 WAN/LAN 网卡配置、地址规划或路由拓扑。
- 不自动安装软件包。
- 不自动关闭 NetworkManager、systemd-resolved 或 firewalld。
- 不自动调整 SELinux、sysctl、Cgroup 或内核配置。
- 不启动 Landscape 进程作为探测手段。

## 验收标准

- 满足全部硬性条件的 glibc Linux 主机报告无错误并返回 `0`，非 Debian 的发行版 ID 不产生错误。
- 缺少 Docker/Podman 时只产生警告，仍返回 `0`。
- 缺少 `ip`、`pppd`、BTF 或核心 BPF/Cgroup 能力时产生错误并返回非零值；无法读取配置或系统接口时产生 `unknown` 并返回非零值。
- 发现 `53`、`6300` 或 `6443` 端口冲突时产生错误，并尽可能显示占用者。
- 非 root、非 Linux 或低于内核 `6.9` 时产生错误。
- 检查过程中不修改任何系统文件或服务状态，不短暂监听或占用被检查端口。
- 检查结果可被未来 `install` 调用，而不需要解析 CLI 文本。
