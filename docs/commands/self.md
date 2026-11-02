# `lkit self`

管理 lkit 自身:安装/升级/移除全局常驻 daemon。lkit CLI 二进制(`/usr/local/bin/lkit`)
由 `install.sh` 安装,不属于本命令管理范围;`self upgrade` 会更新它。

lkit 自身与 landscape 安装完全解耦:daemon 不绑定任何 landscape 根,恢复目标固定为
lkit 地盘(`/root/.lkit/`),见[安装布局与状态](../deployment/layout-and-state.md)。
因此 `self remove` 不要求 landscape 已卸载,`uninstall` 也不影响 daemon。

```text
lkit self install [--flare-psk-file <PATH>]
lkit self upgrade [--include-prerelease] [--version <TAG>]
lkit self remove
```

`upgrade` 的别名是 `update`(`lkit self update` 等价于 `lkit self upgrade`);
顶层 `lkit update` 更新的是 landscape 托管安装,与这里的自身升级无关。

`self` 命令都不接收 `--install-dir`。

daemon 全局唯一(`lkit.service` 单例)。daemon 进程写 pidfile 到
`/root/.lkit/run/lkit.pid`(`0600`,原子替换);同地盘已存在存活实例时拒绝启动。

## install

把 lkit 注册为全局常驻服务:

1. 获取安装锁;
2. 按主机探测结果使用 systemd、OpenRC 或 SysVinit;
3. 校验 `/usr/local/bin/lkit` 可执行,把对应服务定义渲染到全局目录
   `/usr/local/lib/lkit/lkit.service`。systemd unit 为 `0600`；init 脚本为可执行的 `0755`，
   各后端均运行 `/usr/local/bin/lkit daemon`，不绑定 Landscape 安装根；
4. （可选）供给 急救恢复码:提供 `--flare-psk-file`(root 所有、`0400`/`0600`
   私密文件)时,在启动 daemon 前把 psk 写入 lkit 地盘 `config.toml` 的 `[flare]` 段
   (保留既有字段,文件缺失时创建带默认仓库来源的最小配置),daemon 首启即用该 psk
   托管 flare 服务;未提供时保留既有 `[flare]` 配置,交互终端会提示输入 psk(带用途
   说明,至少 12 字符),无终端环境(脚本/控制台后台部署)回落 daemon 自动生成,
   不阻断部署。psK 用途与查看方式见 [flare 协议](../flare/protocol.md);
5. 注册、启用并启动服务：systemd 注册链接为 `/etc/systemd/system/lkit.service`，
   OpenRC/SysVinit 为 `/etc/init.d/lkit.service`，均指向全局原件；检查 daemon PID 非零。
   后台 init 脚本返回后最多等待 3 秒确认进程出现，不等待 bpffs 准备成功。

重复执行时:若旧 daemon 仍在运行,注册完成后执行 `restart` 使其加载当前二进制;
注册或启动失败时尽力回滚已注册状态并删除定义原件。`install` 不复制二进制——daemon
直接使用 `/usr/local/bin/lkit`,升级由 `self upgrade` 负责。

## upgrade

把 lkit CLI 与 daemon 一起升级到目标版本(默认最新 stable):

1. 获取安装锁;
2. 解析目标版本：默认使用 GitHub `releases/latest`，只接受正式版；
   `--include-prerelease` 允许从 Release 列表自动选择包含预发布版本（如 RC）在内的最高
   版本；`--version <TAG>` 显式指定版本并优先于自动选择（如 `--version v0.7.1-rc2`），
   显式指定候选版不需要再加 `--include-prerelease`；
3. 下载对应架构静态二进制（`lkit-<arch>`，arch 为 `x86_64`、`aarch64`、`riscv64` 或 `loongarch64`）与 `SHA256SUMS`,校验 SHA-256,
   与 `install.sh` 同源同校验规则(见[自发布](../release/lkit.md));
4. 版本对比:当前版本与目标版本相同 → 输出提示并返回 `0`,不修改任何文件;
5. 下载并校验成功后,先使暂存二进制可执行并运行 `lkit --version` 自检,再原子替换
   `/usr/local/bin/lkit`;下载、校验、自检或替换失败时保留原二进制;
6. 刷新 daemon:若 `lkit.service` 已注册且 `is-active` → `restart` 加载新二进制;
   已注册但未运行 → 不启动;未注册 → 仅更新 CLI,并提示可用 `lkit self install`
   安装 daemon。

示例：

```sh
lkit self upgrade                              # 最新正式版
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

`upgrade` 不创建事务、不创建保护备份;daemon 在 `restart` 期间短暂退出,由
服务管理器执行重启。替换运行中的 daemon 二进制是安全的:进程继续使用已打开的
旧 inode,重启后加载新文件。

## remove

停止并注销 lkit daemon,幂等可重复:

1. 获取安装锁;
2. 停止并等待 lkit daemon 退出;
3. disable、注销注册链接并执行刷新;
4. 删除 `/usr/local/lib/lkit/lkit.service` 原件(空目录一并移除)。

`remove` 不删除 `/usr/local/bin/lkit`(CLI 由安装脚本管理),不触碰任何 landscape
安装根与 lkit 地盘元数据(`config.toml`、`backups/`、`transactions/` 保留)。

## daemon 进程

`lkit daemon` 是常驻服务本体,固定读取 lkit 地盘 `/root/.lkit/`:

- pidfile 写入 `/root/.lkit/run/lkit.pid`(`0600`,原子替换);已存在存活实例时拒绝启动;
- 收到 `SIGTERM` / `SIGINT` 后清理 pidfile 并退出;
- 尽力准备 bpffs，失败不阻断 daemon/flare，见[服务启动](../service/runtime-and-health.md#daemon-启动与-bpffs)；
- OpenRC 将 daemon 输出保存在 root-only 的 `/var/log/lkit.log`（`0600`），其中可能包含初次生成的恢复码；
- **周期中断恢复**:每 2 秒尝试以非阻塞方式获取安装锁,锁空闲且存在未完成
  事务时,执行与 CLI 相同的 `recover_interrupted` 语义——CLI 因 SSH 断开或
  崩溃消失后,遗留事务由 daemon 自动接管(失败激活回滚、中断恢复、卸载前向
  完成等,详见[事务与中断恢复](../deployment/transactions-and-recovery.md));
  恢复目标从 lkit 地盘的状态与事务发现 landscape 根;
- **恒常托管 flare 服务端**:Linux 上 daemon 启动即托管 Landscape Terrain
  (L2 防失联通道)服务端,`[flare]` 段缺失或无 psk 时生成随机 psk 并持久化;
  每周期对比配置指纹,变更时重启 flare 任务拾取新配置,见[flare 协议](../flare/protocol.md);
- 网络接管待确认阶段仍由 `lkit network confirm|rollback` 人工处理,daemon 不代替确认;
- CLI 命令持有安装锁期间 daemon 自动让行,不产生并发冲突。

## 退出码

- `0`:成功;
- `2`:参数错误(如没有可用的服务管理器、`--version` 非法);
- `1`:其他失败。

`upgrade` 版本相同或已是最新时返回 `0`。
