# 环境检查规格

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit
- 依赖：[03-lifecycle.md](./03-lifecycle.md)（权限与路径发现）、[05-architecture.md](./05-architecture.md)（退出码）、[10-install-flow.md](./10-install-flow.md)（安装状态机）

## 2. 概述

`lkit` 在执行各类操作前需对主机环境进行预检，分为三层：

| 层次 | 触发时机 | 失败语义 |
|------|---------|---------|
| **Precheck**（安装前预检） | `lkit install` 状态机 Phase 1 | Hard：直接退出，不落地任何文件 |
| **Pre-operation**（命令级前置检查） | 非 install 命令（`status` / `backup` / `upgrade` 等）执行前 | 按命令需求判定，不可满足时退出 |
| **Diagnose**（运行时诊断） | `lkit diagnose` | 只读展示，不阻止任何操作 |

检查项分为三级：

| 级别 | 含义 | 失败行为 |
|------|------|---------|
| Hard | 必须满足 | 报错退出，不继续 |
| Soft | 建议满足 | 警告后可继续 |
| Interactive | 需要用户选择 | TTY 模式下由用户决策；非 TTY 模式下按内置默认行为执行 |

## 3. 安装前预检（Precheck）

Precheck 对应安装状态机 Phase 1，按如下顺序依次执行。Hard 项失败时立即退出，Soft 项失败仅警告，Interactive 项在 Wizard 中弹出交互。

### 3.1 Kernel 版本

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `uname -r` 解析主版本号，需 ≥ 6.9 |
| **失败行为** | 退出码 6.1，提示升级内核 |
| **降级条件** | — |

Landscape 使用 eBPF TC hook 进行全部数据包处理，Kernel 6.9+ 提供必需的 BPF 功能和 BTF 支持。

检测时提取 `uname -r` 输出的 `<major>.<minor>` 格式版本号，与 6.9 做数值比较。

### 3.2 Kernel BTF / BPF 内核特性

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | 检查以下条件全部满足 |
| **失败行为** | 退出码 6.2，提示内核缺失的编译选项 |
| **降级条件** | — |

所需内核编译选项：

| 选项 | 要求值 | 说明 |
|------|--------|------|
| `CONFIG_BPF` | `y` | BPF 框架 |
| `CONFIG_BPF_SYSCALL` | `y` | BPF 系统调用 |
| `CONFIG_BPF_JIT` | `y` 或未设置 | BPF JIT 编译器。未设置时回退到解释器，性能下降但功能不受影响 |
| `CONFIG_BPF_JIT_DEFAULT_ON` | `y` 或未设置 | BPF JIT 默认开启。未设置时可通过 sysctl 手动开启 |
| `CONFIG_BPF_UNPRIV_DEFAULT_OFF` | `y` 或未设置 | 非特权用户 BPF 默认关闭。仅安全加固选项，Landscape 以 root 运行不受影响 |
| `CONFIG_BPF_LSM` | `y` | BPF Linux Security Module |
| `CONFIG_CGROUP_BPF` | `y` | cgroup BPF |
| `CONFIG_BPF_EVENTS` | `y` | BPF 事件追踪 |
| `CONFIG_BPF_STREAM_PARSER` | `y` | BPF 流解析器 |
| `CONFIG_LWTUNNEL_BPF` | `y` | BPF 轻量级隧道 |
| `CONFIG_NET_CLS_BPF` | `y` 或 `m` | BPF TC classifier |
| `CONFIG_NET_ACT_BPF` | `y` 或 `m` | BPF TC action |
| `CONFIG_NETFILTER_BPF_LINK` | `y` | netfilter BPF link |
| `CONFIG_NETFILTER_XT_MATCH_BPF` | `y` 或 `m` | netfilter BPF match |
| `CONFIG_IPV6_SEG6_BPF` | `y` | IPv6 SRv6 BPF |
| `CONFIG_DEBUG_INFO_BTF` | `y` | BTF 类型信息，eBPF 程序加载必需 |
| `CONFIG_NET_SCH_INGRESS` | `y` | TC ingress hook |

注：其中 `NET_CLS_BPF`、`NET_ACT_BPF`、`NETFILTER_XT_MATCH_BPF` 编译为内核模块（`=m`）也可满足要求，运行时自动加载。

检测时：未找到或未设置时列出具体缺失项。其中 `BPF_JIT`、`BPF_JIT_DEFAULT_ON`、`BPF_UNPRIV_DEFAULT_OFF` 三项为性能优化和安全加固选项，缺失仅警告；其余缺失时报错退出（退出码 6.2）。

检测方式：

1. 检查 `/sys/kernel/btf/vmlinux` 是否存在
2. 依次从 `/proc/config.gz`、`/boot/config-$(uname -r)`、`/lib/modules/$(uname -r)/config` 读取内核配置
3. 匹配上表中的选项，`=y` 或 `=m` 视为通过

若均无法读取内核配置但 `/sys/kernel/btf/vmlinux` 存在，视为通过（最直接的证据）。

### 3.3 root 权限

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `geteuid() == 0` |
| **失败行为** | 退出码 2，提示使用 `sudo lkit install` |
| **降级条件** | — |

Landscape 需要 `CAP_BPF`、`CAP_NET_ADMIN`、`CAP_NET_RAW`、`CAP_SYS_ADMIN` 等特权。V1 默认以 root 运行。退出码 2 对应 `05-architecture.md §8`。

### 3.4 systemd 可用性

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `systemctl --version` exit code 0 |
| **失败行为** | 退出码 6.3，提示 V1 仅支持 systemd |
| **降级条件** | — |

Landscape Kit V1 服务管理仅支持 systemd（见 `03-lifecycle.md §6.6`）。非 systemd 系统（如 OpenRC、BusyBox init）不在 V1 支持范围内。

### 3.5 iproute2（`ip` 命令）

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `ip --version` exit code 0 |
| **失败行为** | 退出码 6.4，提示 `apt install iproute2` 或等效包管理器命令 |
| **降级条件** | — |

`ip` 是 Landscape 最核心的外部命令，用于接口管理（`ip link`）、路由管理（`ip route`）、地址管理（`ip addr`）、网络命名空间（`ip netns`）等全部网络操作场景。不可替代。

### 3.6 `iw`（无线工具）

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅 WiFi 场景） |
| **检测方式** | `iw --version` exit code 0 |
| **失败行为** | 警告，建议 `apt install iw` |
| **降级条件** | 系统无无线网卡时跳过 |

用于设置 WiFi 接口模式（managed / AP）。

### 3.7 `hostapd`

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅 WiFi AP 场景） |
| **检测方式** | `hostapd -v` exit code 0 |
| **失败行为** | 警告，建议 `apt install hostapd` |
| **降级条件** | 无 WiFi AP 需求时跳过 |

### 3.8 `pppd`（PPP 守护进程）

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅 PPPoE 场景） |
| **检测方式** | `pppd --version` exit code 0；同时检查 `/etc/ppp/peers/` 目录是否存在 |
| **失败行为** | 警告，建议 `apt install ppp` |
| **降级条件** | 不使用 PPPoE WAN 时跳过 |

Landscape 在 `landscape-common/src/iface/ppp.rs` 中向 `/etc/ppp/peers/` 写入配置文件，目录必须存在。

### 3.9 `docker`（容器运行时）

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅容器管理功能） |
| **检测方式** | `docker --version` exit code 0，或 Docker socket（`/var/run/docker.sock`）可达 |
| **失败行为** | 警告，容器管理功能不可用 |
| **降级条件** | 禁用容器功能时跳过 |

### 3.10 端口 53（UDP）

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅警告） |
| **检测方式** | `ss -ulpn 'sport = :53'` 或等效方法 |
| **失败行为** | 打印警告信息，列出占用进程，不阻止安装 |
| **降级条件** | Landscape 可配置为使用其他端口 |

Landscape 内置 DNS 服务器（`landscape-dns`）默认监听 UDP 53。检测到占用时输出：

```
⚠ 端口 53 (UDP) 已被占用
  占用进程: <进程名> (PID <pid>)
  Landscape 需要端口 53 用于内置 DNS 服务
  请确认该服务已停止，或 Landscape 配置使用其他端口
```

仅做信息提示，不停止安装、不做任何系统修改。

### 3.11 端口 6300 / 6443（TCP）

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `ss -tln 'sport = :6300'`、`ss -tln 'sport = :6443'` |
| **失败行为** | 退出码 6.5，提示占用进程 |
| **降级条件** | — |

6300 为 HTTP 重定向端口，6443 为 Web UI HTTPS 端口。

### 3.12 端口 6053（TCP，DoH）

| 字段 | 值 |
|------|-----|
| **级别** | Soft（仅 DoH 启用时） |
| **检测方式** | `ss -tln 'sport = :6053'` |
| **失败行为** | 警告 |
| **降级条件** | DoH 未启用时跳过 |

### 3.13 NetworkManager 冲突

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `systemctl is-active NetworkManager` 返回 `active` |
| **失败行为** | 退出码 6.6，提示停止并卸载 NetworkManager |
| **降级条件** | — |

Landscape 直接通过 `ip` 命令和 eBPF TC hook 管理网络接口，不与 NetworkManager 协作。两套网络管理栈同时操作同一网卡会造成冲突。

检测到 NetworkManager 正在运行时立即退出：

```
✗ NetworkManager 正在运行
  Landscape 直接管理网络接口，与 NetworkManager 不兼容。
  请手动执行：
    systemctl stop NetworkManager
    systemctl disable NetworkManager
  # 建议卸载：
    apt purge network-manager
  完成后重新运行 lkit install
```

### 3.14 SELinux Enforcing

| 字段 | 值 |
|------|-----|
| **级别** | Interactive（TTY）；Soft（非 TTY） |
| **检测方式** | `getenforce` 返回 `Enforcing` |
| **失败行为** | TTY：Wizard 中弹出提示，询问是否继续；非 TTY：警告后继续 |
| **降级条件** | SELinux 为 Disabled / Permissive 时跳过 |

Landscape 需要大量系统特权操作，SELinux Enforcing 模式可能阻止以下行为：

| 操作 | 需要的权限 |
|------|-----------|
| 加载 eBPF 程序、操作 eBPF map | `bpf` 权限类 |
| 管理网络接口、路由 | `netlink_route_socket`、`net_admin` |
| 原始套接字（PPPoE 等） | `rawip_socket` |
| 写入 `/etc/resolv.conf` | 文件写入权限 |
| 写入 `/sys/fs/bpf/` | 文件系统权限 |
| 操作 `/etc/ppp/peers/` | 文件写入权限 |

在 Wizard 中显示上述信息，并提供参考放行命令：

```
# 方式一：将 landscape 设为 permissive domain（简便）
semanage permissive -a landscape_t

# 方式二：检查当前 avc 拒绝日志并生成策略
grep landscape /var/log/audit/audit.log | audit2allow -M landscape
semodule -i landscape.pp
```

询问用户「确认继续吗？」用户确认后继续安装，记录到安装报告。

### 3.15 RLIMIT_MEMLOCK

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `ulimit -l` 返回 `unlimited` |
| **失败行为** | 退出码 6.7，提示设置 |
| **降级条件** | — |

eBPF 程序加载和 map 分配需要锁定内存。需在 systemd unit 中设置 `LimitMEMLOCK=infinity`。检查当前进程的 memlock limit：

```
✗ RLIMIT_MEMLOCK 不足
  eBPF 加载需要锁定大量内存页。
  确认 systemd unit 中含有:
    LimitMEMLOCK=infinity
  或临时设置:
    ulimit -l unlimited
```

### 3.16 `/sys/fs/bpf/` 可写

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | 尝试在 `/sys/fs/bpf/landscape/` 创建临时文件（`access(F_OK|W_OK)`） |
| **失败行为** | 报错退出，提示 BPF 文件系统未挂载 |
| **降级条件** | — |

Landscape 将 eBPF map pin 到 `/sys/fs/bpf/landscape/`。若目录不存在或无写权限，检查 bpf 文件系统是否已挂载：

```
mount -t bpf
# 若未挂载：
mount -t bpf bpf /sys/fs/bpf/
```

### 3.17 磁盘空间

| 字段 | 值 |
|------|-----|
| **级别** | Soft |
| **检测方式** | `statvfs` 计算 Landscape HOME 所在分区的可用空间 |
| **失败行为** | 可用空间 < 500MB 时警告 |
| **降级条件** | — |

### 3.18 重复安装检测

| 字段 | 值 |
|------|-----|
| **级别** | Hard |
| **检测方式** | `landscape_init.lock` 文件存在性检查 |
| **失败行为** | 拒绝，提示使用 `--force` 覆盖安装 |
| **降级条件** | `--force` 参数覆盖 |

已有实现，位于 `lkit-cli/src/commands/install.rs`。详见 `03-lifecycle.md §5.2`。

## 4. 命令级前置检查

非 `install` 命令在执行前需通过一系列前置检查。检查项按命令不同而异。

### 4.1 公共前置检查

| 检查项 | 检测方式 | 失败行为 |
|--------|---------|---------|
| Landscape HOME 可发现 | 按 `03-lifecycle.md §2.1` 的发现顺序，最终路径可达 | 退出码 3 |
| 已安装判定 | `landscape_init.lock` 存在 | 未安装时大多数操作拒绝 |
| 权限判定 | `geteuid() == 0` vs 普通用户 | 按命令需求报错 |
| 并发锁检测 | `{manager_home}/runtime/lkit.pid` 存在 | 退出，提示已有实例运行 |

### 4.2 命令级权限矩阵

| 命令 | HOME 可发现 | 已安装 | 权限 | 并发锁 | 备注 |
|------|------------|--------|------|--------|------|
| `install` | — | Hard（拒绝重复） | Hard（root） | Hard | `--force` 可覆盖已安装 |
| `status` | Soft（降级为本机 systemd 状态） | Soft | — | — | 无 HOME 时仅展示本机 systemd 状态 |
| `service` | — | — | Hard（root） | — | 仅依赖 systemd，与 Landscape 安装无关 |
| `backup create` | Hard | Hard | Soft（HOME 可读） | Hard | — |
| `backup list` | — | — | Soft（HOME 可读） | — | 仅依赖备份目录（manager_paths） |
| `backup restore` | — | — | Hard（root） | Hard | 备份包自包含全部文件，不依赖已安装的 landscape |
| `backup delete` | — | — | Soft（HOME 可读） | — | 仅依赖备份目录（manager_paths） |
| `upgrade check` | Hard | Hard | Soft（HOME 可读） | — | — |
| `upgrade apply` | Hard | Hard | Hard（root） | Hard | — |
| `rollback list` | Hard | Hard | Soft（HOME 可读） | — | — |
| `rollback apply` | Hard | Hard | Hard（root） | Hard | — |
| `logs` | Hard | Hard | Soft（日志文件可读） | — | — |
| `diagnose` | Soft（无 HOME 做系统级检查） | Soft | — | — | 见第 5 章 |
| `config export` | Hard | Hard | Soft（HOME + API 可达） | — | — |
| `self version` | — | — | — | — | 无任何前置检查 |
| `self upgrade check` | — | — | — | — | — |
| `mirror *` | — | — | — | — | 独立子命令，无前置检查 |

### 4.3 权限不足时的处理

权限不足时输出明确的错误信息和建议的提权方式（如 `sudo lkit <command>`），不尝试自动提权。详见 `03-lifecycle.md §3.3`。

## 5. 运行时诊断（`lkit diagnose`）

`lkit diagnose` 执行只读健康检查，覆盖安装后 Landscape 的运行状态评估。

### 5.1 检查项

| # | 检查项 | 状态 | 备注 |
|---|--------|------|------|
| 1 | systemd 服务状态 | ✅ 已有 | 通过 `ServiceManager::status()` 检查 `landscape.service` 是否 active |
| 2 | HOME 目录完整性 | ✅ 已有 | 检查 `landscape.toml`、`landscape_db.sqlite`、`static/` 目录存在性 |
| 3 | API 可达性 | ✅ 已有 | 通过 `LkitClient::health_check()` 发起 HTTP 请求 |
| 4 | 最近错误日志摘要 | ✅ 已有 | 读取 Landscape 日志文件尾部 |
| 5 | 磁盘空间 | 📝 新增 | 复用 Precheck §3.17 的逻辑 |
| 6 | 系统命令存在性 | 📝 新增 | 检查 `ip`、`iw`、`pppd` 等关键命令 |
| 7 | NetworkManager 状态 | 📝 新增 | 只读展示是否运行中 |
| 8 | SELinux 模式 | 📝 新增 | 只读展示当前模式（Enforcing / Permissive / Disabled） |
| 9 | Kernel 版本 | 📝 新增 | 只读展示当前版本 |

### 5.2 数据模型

`lkit-core` 中已有 `DiagnosticCheck` 和 `DiagnosticResult` 模型（`crates/lkit-core/src/models.rs`）：

```rust
pub struct DiagnosticResult {
    pub checks: Vec<DiagnosticCheck>,
}

pub struct DiagnosticCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}
```

新增的检查项沿用此模型。

### 5.3 命令分类

| 模式 | 行为 |
|------|------|
| `lkit diagnose` | 表格输出所有检查项的通过/失败状态 |
| `lkit diagnose --json` | JSON 格式输出 `DiagnosticResult` |
| 无 Landscape HOME | 降级为仅系统级检查（版本、命令、SELinux、NM） |

### 5.4 退出码

| 条件 | 退出码 |
|------|--------|
| 全部通过 | 0 |
| 仅 API 检查失败 | 4（网络不可达，复用 `05-architecture.md §8`） |
| 其他检查失败 | 1 |
| 无 Landscape HOME | 3（仅系统级检查可执行） |

## 6. 失败语义与退出码

### 6.1 Precheck 退出码细分

扩展 `05-architecture.md §8` 的退出码 6（系统依赖不满足）：

| 码 | 含义 |
|----|------|
| 6 | 系统依赖不满足（通用） |
| 6.1 | Kernel 版本不足（< 6.9） |
| 6.2 | Kernel BTF / BPF 内核特性缺失 |
| 6.3 | systemd 不可用 |
| 6.4 | `ip`（iproute2）不可用 |
| 6.5 | 端口 6300 或 6443 冲突 |
| 6.6 | NetworkManager 冲突 |
| 6.7 | RLIMIT_MEMLOCK 不足 |

### 6.2 错误输出格式

所有 Precheck 失败统一使用以下格式：

```
Error: <简短描述>
Check: <检查项名>
Suggestion: <建议操作>
```

## 7. 代码组织

```
lkit-core/src/check/            # 新增模块
  mod.rs                        # CheckResult, CheckLevel 类型定义
  kernel.rs                     # kernel_version(), check_btf(), check_bpf_config()
  commands.rs                   # check_command(name) -> bool
  ports.rs                      # check_port_available(port, proto) -> CheckResult
  selinux.rs                    # check_selinux_mode() -> SelinuxStatus
  network_manager.rs            # check_network_manager_active() -> bool

lkit-app/src/install/
  precheck.rs                   # PrecheckUseCase：编排所有 Precheck 检查，按级别执行

lkit-app/src/diagnose/
  mod.rs                        # 扩展 DiagnoseUseCase，复用 check/ 模块

lkit-cli/src/commands/
  install.rs                    # 调用 PrecheckUseCase（已有入口点）
  diagnose.rs                   # 扩展输出项（已有入口点）

lkit-cli/src/wizard/steps/
  precheck.rs                   # Wizard 交互：SELinux 确认提示
```

### 7.1 PrecheckUseCase 接口

```rust
pub struct PrecheckUseCase {
    system_target: SystemTarget,
    config: CollectedConfig,          // 包含是否启用 WiFi / PPPoE / 容器
}

pub struct PrecheckResult {
    pub hard_failures: Vec<CheckFailure>,
    pub warnings: Vec<CheckWarning>,
    pub interactive: Vec<InteractiveCheck>,
}

pub async fn run(&self) -> Result<PrecheckResult, AppError>;
```

### 7.2 复用关系

- `check::kernel`、`check::commands`、`check::ports` 模块被 `PrecheckUseCase` 和 `DiagnoseUseCase` 共用
- `DiagnosticCheck` 模型（`lkit-core`）作为检查结果的统一承载类型
- 系统架构/libc 检测复用 `lkit-core/src/system_detect.rs`（已有实现）
