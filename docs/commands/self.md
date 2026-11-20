# `lkit self`

管理 lkit 自身:安装/升级/移除全局常驻 daemon。lkit CLI 二进制(`/usr/local/bin/lkit`)
由 `install.sh` 安装,不属于本命令管理范围;`self upgrade` 会更新它。

lkit 自身与 landscape 安装完全解耦:daemon 不绑定任何 landscape 根,恢复目标固定为
lkit 地盘(`/root/.lkit/`),见[安装布局与状态](../deployment/layout-and-state.md)。
因此 `self remove` 不要求 landscape 已卸载,`uninstall` 也不影响 daemon。

```text
lkit self install [--flare-psk-file <PATH>]
lkit self restart
lkit self upgrade [--include-prerelease] [--version <TAG>]
lkit self remove
```

`upgrade` 的别名是 `update`(`lkit self update` 等价于 `lkit self upgrade`);
顶层 `lkit update` 更新的是 landscape 托管安装,与这里的自身升级无关。

`self` 命令都不接收 `--install-dir`。

daemon 全局唯一(`lkit.service` 单例)。init 托管监管进程，监管进程启动实际 daemon；
退避采用 1s、3s、5s、10s 封顶且无限重试，稳定运行 5 分钟后重置，
主动停止取消拉起，见[监管契约](../network/daemon-recovery.md#启动与生命周期)。
daemon 进程写 pidfile 到
`/root/.lkit/run/lkit.pid`(`0600`,原子替换);同地盘已存在存活实例时拒绝启动。

## install

把 lkit 注册为全局常驻服务:

1. 获取安装锁;
2. 按主机探测结果使用 systemd、OpenRC 或 SysVinit;
3. 校验 `/usr/local/bin/lkit` 可执行，检查服务注册归属。外部文件或指向其他原件的注册链接
   在任何写入和服务动作前拒绝；定义原件不得是符号链接。未注册但已有原件时，须通过
   受管定义校验。把对应服务定义渲染到全局目录
   `/usr/local/lib/lkit/lkit.service`。systemd unit 为 `0600`；init 脚本为可执行的 `0755`，
   各后端均运行 `/usr/local/bin/lkit daemon --supervise`，不绑定 Landscape 安装根；
4. 可用 `--flare-psk-file` 从 root 所有的 0400 或 0600 文件设置私有发现密钥。省略时保留既有设置，新装默认公开发现。设备指纹和登录方式见 [Flare 协议](../flare/protocol.md)。

5. 注册、启用并启动服务：systemd 注册链接为 `/etc/systemd/system/lkit.service`，
   OpenRC/SysVinit 为 `/etc/init.d/lkit.service`，均指向全局原件；检查监管进程存在，且实际 daemon 的新 PID 与恢复能力标识匹配。
   后台 init 脚本返回后最多等待 3 秒确认进程出现，不等待 bpffs 准备成功。

重复执行时，旧 daemon 正在运行则执行 `restart`；原本停止则启动。修改前保存定义内容、
权限、启用与运行状态，以及各后端的启动成员。systemd 区分持久和运行时成员，失败
恢复不把临时启用改为永久启用；OpenRC/SysVinit 保留完整启动链接。失败时恢复已有受管定义及
服务状态，不注销原服务。指向受管路径但原件缺失的注册允许通过 `self install` 修复；
该修复失败时保留新生成的有效定义，并恢复原启用和运行设置，供重试使用。
首次安装失败只清理本次创建且仍属于 lkit 的注册和定义；
停止、注销或刷新失败时保留原件并报告清理错误，供排除故障后重试。
显式供给的 Flare 恢复码保留在配置中，不因服务安装失败而改回旧值。
`install` 不复制二进制，daemon 使用 `/usr/local/bin/lkit`，升级由 `self upgrade` 负责。

## restart

以 root 身份运行 `lkit self restart` 可让 daemon 加载当前安装的二进制，例如在 `just push` 之后。
命令通过主机的 systemd、OpenRC 或 SysVinit 校验已有服务的注册归属和定义，重启运行中的服务或启动已停止的服务，并等待新的 daemon PID 和恢复能力标识就绪。开机启动设置保持原值。首次部署请使用 `lkit self install`。

## upgrade

把 lkit CLI 与 daemon 一起升级到目标版本(默认最新 stable):

1. 获取安装锁；服务管理器可用时先校验注册归属和定义路径类型，外部注册在替换二进制前拒绝；
2. 解析目标版本：默认使用 GitHub `releases/latest`，只接受正式版；
   `--include-prerelease` 允许从 Release 列表自动选择包含预发布版本（如 RC）在内的最高
   版本；`--version <TAG>` 显式指定版本并优先于自动选择（如 `--version v0.7.1-rc2`），
   显式指定候选版不需要再加 `--include-prerelease`；
3. 下载前比较版本：自动选择只允许高于当前版本的目标，不自动降级；相同或更低时
   输出所选渠道没有更新的提示并返回 `0`，不请求校验清单或二进制，也不替换文件。
   当前为 RC 而正式渠道版本更低时提示可使用 `--include-prerelease`。更高版本的正式版
   仍可正常升级；显式 `--version` 允许有意指定旧版本，同版本仍为 no-op；
4. 下载对应架构静态二进制（`lkit-<arch>`，arch 为 `x86_64`、`aarch64`、`riscv64` 或 `loongarch64`）与 `SHA256SUMS`,校验 SHA-256,
   与 `install.sh` 同源同校验规则(见[自发布](../release/lkit.md));
5. 下载并校验成功后,先使暂存二进制可执行并运行 `lkit --version` 自检,再原子替换
   `/usr/local/bin/lkit`;下载、校验、自检或替换失败时保留原二进制。自检启动失败明确
   标注暂存文件路径，提示检查目标架构及解释器／动态加载器，避免仅报告裸 I/O 错误；
6. 刷新前再次校验归属；若 `lkit.service` 指向 lkit 全局原件且 `is-active` → `restart` 加载新二进制;
   已注册但未运行 → 不启动;未注册 → 仅更新 CLI,并提示可用 `lkit self install`
   安装 daemon。

示例：

```sh
lkit self upgrade                              # 检查更高版本的正式版，不自动降级
lkit self upgrade --include-prerelease         # 包含 RC 等预发布版本的最新版本
lkit self upgrade --version v0.7.1-rc2          # 精确指定候选版
```

自动预发布选择会分页读取 GitHub Release 列表，不依赖列表顺序或发布时间；排除 draft
与非法版本 tag，忽略 build metadata 比较版本优先级。同一基础版本的正式版优先于候选版，
更高基础版本的候选版可以被选中。兼容 `rcN` 和 `rc.N`，RC 序号按数字比较（`rc10` 高于
`rc2`）；预发布 tag 必须以数字发布序号结尾。最多读取 100 页、每页 100 条，超限或任一
页请求/解析失败时报告错误，不使用不完整列表，不下载或替换二进制。
默认模式没有正式 Release 时直接报错，不自动回退 RC。显式目标也不允许 draft，API
返回的 tag 必须与请求版本一致。

`--include-prerelease` 只影响本次 `self upgrade` / `self update` 的自动选择，不保存到
配置，不影响安装脚本、`self install` 或 Landscape 的顶层 `update`。

`upgrade` 遇到未完成事务时拒绝降级。保留的事务历史含精确 systemd 启用快照时，
即使事务已终结，也在下载资产和替换二进制前拒绝降级，避免旧 daemon 无法读取历史；
历史记录损坏或路径类型异常时同样拒绝，不删除记录来绕过检查。
已有旧服务入口通过 `self install` 刷新为监管入口。
Landscape 服务定义由 `lkit repair service` 管理；升级 lkit 二进制不自动迁移它。
旧安装的 systemd 注册仍直接指向安装盘时，应在整机重启前执行该修复，使启动定义在
安装盘尚未挂载时可被 systemd 读取。修复保留 Landscape 的 PID、启用状态和运行状态。
`upgrade` 不创建事务、不创建保护备份;daemon 在 `restart` 期间短暂退出,由
服务管理器执行重启。替换运行中的 daemon 二进制是安全的:进程继续使用已打开的
旧 inode,重启后加载新文件。

## remove

停止并注销 lkit daemon,幂等可重复:

1. 获取安装锁;
2. 校验服务注册归属；存在外部注册时拒绝操作，不停止或禁用外部服务；
3. 停止并等待 lkit daemon 退出，disable、注销注册链接并执行刷新；
4. 删除 `/usr/local/lib/lkit/lkit.service` 原件(空目录一并移除)。

服务查询、禁用、注销或刷新失败时返回非零退出码，保留定义原件供排除故障后重试。
注册已不存在时刷新管理器并清理原件，重复移除返回成功。

`remove` 不删除 `/usr/local/bin/lkit`(CLI 由安装脚本管理),不触碰任何 landscape
安装根与 lkit 地盘元数据(`config.toml`、`backups/`、`transactions/` 保留)。Landscape
的 systemd 启动副本、注册和启用状态同样保留；它们属于 Landscape 安装。

存在未完成事务时 `self remove` 拒绝移除 daemon，须先完成事务恢复。
已提交的 Landscape 不以 daemon 存活为持续运行条件；移除 daemon 后，服务启动仍
调用保留的 lkit CLI 执行恢复检查。启用的 systemd 服务在整机重启时从系统盘启动副本
读取定义，等待安装根挂载后正常启动，不需要 daemon 补拉起。手工停止的服务不会因
daemon 重启而被主动启动。需要新建接管、reinit 或委托修改时，必须先重新
部署 daemon。手工删除 `/usr/local/bin/lkit` 会导致受管 Landscape 的启动检查失败。

## daemon 进程

`lkit daemon` 是常驻服务本体,固定读取 lkit 地盘 `/root/.lkit/`:

- daemon 持有 `/root/.lkit/run/daemon.lock` 独占锁，重复启动被锁拒绝；取得锁后原子覆盖 pidfile（`/root/.lkit/run/lkit.pid`，`0600`），残留或被复用的 PID 不阻止恢复；
- 运行状态与能力检查同时要求实例锁被持有、PID 为有效正数且进程存活；能力标识还须与主 PID 匹配；
- 收到 `SIGTERM` / `SIGINT` 后清理 pidfile 并退出;
- 尽力准备 bpffs，失败不阻断 daemon/flare，见[服务启动](../service/runtime-and-health.md#daemon-启动与-bpffs)；
- OpenRC 将 daemon 输出保存在 root-only 的 `/var/log/lkit.log`（`0600`），其中可能包含初次生成的恢复码；
- **周期中断恢复**:每 2 秒尝试以非阻塞方式获取安装锁,锁空闲且存在未完成
  事务时,执行与 CLI 相同的 `recover_interrupted` 语义——CLI 因 SSH 断开或
  崩溃消失后,遗留事务由 daemon 自动接管(失败激活回滚、中断恢复、卸载前向
  完成等,详见[事务与中断恢复](../deployment/transactions-and-recovery.md));
  恢复目标从 lkit 地盘的状态与事务发现 landscape 根;
- **恒常托管 flare 服务端**:Linux 上 daemon 启动即托管 Landscape Terrain
  (L2 防失联通道)服务端,新装默认公开发现，已有 PSK 的配置保留私有发现;
  每周期对比配置指纹,变更时重启 flare 任务拾取新配置,见[flare 协议](../flare/protocol.md);
- 网络接管由 daemon 监测期限与主机重启并自动回滚；用户确认仍通过 `lkit network confirm`；
  见[恢复契约](../network/daemon-recovery.md)；
- CLI 命令持有安装锁期间 daemon 自动让行,不产生并发冲突。

## 退出码

- `0`:成功;
- `2`:参数错误(如没有可用的服务管理器、`--version` 非法);
- `1`:其他失败。

`upgrade` 版本相同或已是最新时返回 `0`。
