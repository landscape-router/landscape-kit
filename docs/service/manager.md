# 服务管理器抽象

`lkit` 通过 `ServiceManager` trait 抽象主流发行版 init 系统的服务操作。已实现
后端:`systemd`、`openrc`(Alpine/Gentoo)、`sysvinit`(Debian/RedHat 的
update-rc.d 环境)。runit 等后端按需接入。

## 设计原则

- 契约只暴露 lkit 对服务的操作需求,不绑定任何具体 init 系统的概念;
- init 系统特有的概念(unit 名、MainPID 查询、daemon-reload、mask、注册软链接细节、
  rc-service/update-rc.d 调用)全部留在后端内部,不进 trait;
- 接入新后端时以真实操作驱动契约演进,不做投机抽象;
- 后端必须满足 `Send + Sync`,可放入 `InstallRuntime.service_manager` 的
  `Box<dyn ServiceManager>`。

## trait 契约

```rust
trait ServiceManager: Send + Sync {
    fn kind(&self) -> ServiceManagerKind;                 // systemd / openrc / sysvinit
    fn probe(&self) -> Availability;                       // Available / NotDetected / Unavailable

    fn service_name(&self, service: ManagedService) -> &str;

    fn render_definition(&self, service, root) -> Result<String>;
    fn validate_definition(&self, service, content, root) -> Result<()>;

    fn query_registration(&self, service) -> Result<SystemRegistration>;
    fn register(&self, service, origin) -> Result<()>;
    fn unregister(&self, service, origin) -> Result<()>;

    fn is_enabled / enable / disable;
    fn capture_enablement(service) -> Result<Enablement>;
    fn restore_enablement(service, origin, before) -> Result<()>;
    fn is_active / active_state / start / stop / restart;
    fn stop_and_wait(service, wait_for_exit) -> Result<()>;   // 默认实现:stop + 轮询
    fn refresh(&self) -> Result<()>;                           // 默认 no-op(systemd 覆盖为 daemon-reload)
    fn refresh_definition(&self, service, origin) -> Result<()>;
    fn definition_needs_refresh(&self, service, origin) -> Result<bool>;

    fn main_pid(&self, service) -> Result<u32>;

    fn restore_registration(service, before, origin) -> Result<()>;
    fn restore_before(service, before, origin) -> Result<()>; // 默认实现:先恢复注册再恢复 active

    fn resolv_conf(&self) -> &Path;                            // 宿主状态,由管理器配置携带
    fn as_any(&self) -> &dyn Any;                              // 向下转型
}
```

共享类型:

- `ServiceManagerKind`:序列化在安装状态(`state/install-state.json` 的
  `service.manager`)与事务文件(`systemd_before`)中。当前变体
  `systemd` / `openrc` / `sysvinit`,新增后端时增加变体并处理状态 schema 演进;
- `ManagedService`:lkit 需要托管的服务身份,当前有 `LandscapeRouter` 与
  `LkitDaemon`(lkit 常驻服务);
- `Availability`:`Available { version }` / `NotDetected`(主机没有运行该 init)/
  `Unavailable(reason)`(看似使用该 init 但环境损坏);
- `SystemRegistration`:系统注册路径的实时状态(`Missing` / `Symlink { target }` /
  `Conflict { file_type }`)；`target` 表示受管定义原件的逻辑路径，systemd 后端负责验证
  启动副本并将物理链接目标映射回该路径；
- `Registration` / `RegistrationKind`:序列化的事务前注册状态;
- `Enablement`:受管服务的启用事实及后端恢复信息；systemd 保存持久和运行时启动成员及捕获时的 boot ID；
- `ServiceBefore`:受管服务事务前状态(注册 + 启用快照 + active),失败回滚时恢复用。

`restore_registration` 恢复注册及启用状态，不启动服务。注册快照为 `Missing` 时执行
注销；为受管链接时先恢复注册，再恢复启用状态。默认 `restore_enablement` 实现在
当前状态与快照一致时不重复启用或禁用，修改后复查结果，不一致时返回错误。`restore_before`
在上述步骤成功后恢复 active；需要先恢复数据的工作流只调用 `restore_registration`，
待原 `current` 和 data 就位后再按原 active 状态启动服务。

OpenRC／SysVinit 的普通受管服务启用快照记录 enabled 布尔值，恢复保证原启用或禁用
状态，不包含任意自定义 runlevel 集合或 S/K 链接顺序。systemd 使用下述独立启用快照。

## 事务前状态捕获

```rust
capture_before(manager: &dyn ServiceManager, service: ManagedService) -> Result<ServiceBefore>
```

通用实现:查询注册、捕获启用快照并查询 active。注册所有权冲突(`Conflict`)时阻断,
不能自动接管。

systemd 的 `enabled` 与 `enabled-runtime` 都表示当前已启用，但恢复必须保留各自的
有效期。快照分别记录 `/etc/systemd/system` 与 `/run/systemd/system` 的受管
`*.wants`／`*.requires` 成员。回滚清理事务期间新增的受管成员并恢复原成员；不会通过
普通 `enable` 将临时启用转为永久启用，也不会覆盖外部链接。持久和临时成员可以同时存在。
运行时成员绑定捕获时的 boot ID，同次主机启动中的 daemon 重启会恢复它们，整机重启
后则视为到期，不重新创建。恢复后的启用状态按仍有效的成员计算。

启用快照具有独立版本；含精确信息的快照不能被仅理解布尔值的 daemon 静默恢复。
读取只含布尔值的兼容快照时保留其原有恢复语义，不推测未记录的成员或临时有效期。
`self install` 的失败恢复也使用该启用契约；OpenRC／SysVinit 保留各自的完整启动链接。

## 后端探测顺序

`InstallRuntime::production` 按 `host_manager()` 顺序探测:

1. `systemd`:PID 1 是 systemd 且 systemctl 可连接;
2. `openrc`:`/etc/init.d` 存在、`rc-service`/`rc-update` 可执行且可应答;
3. `sysvinit`:`/etc/init.d` 存在、`update-rc.d` 可执行。

全部失败时退回 systemd 实例,由工作流的 `require_manager` 对不可用环境报
`UnsupportedPlatform`(退出码 2)。安装状态只接受受支持集合内的后端
(`ServiceManagerKind::supported()`),其他值视为损坏。

## systemd 后端

`service/systemd.rs` 的 `Systemd` 结构体实现该 trait。systemd 专属操作保留为
自由函数,仅供显式要求 systemd 的流程使用:

- `inspect_host_service` / `stop_disable_mask_host_service` / `restore_host_service`
  (network takeover、旧部署迁移、卸载前接管特征警告);
- `unit_command` / `unit_query` / `unit_property` / `fragment_path` /
  `find_units_serving_config_dir` / `daemon_reload`;
- `downcast(manager) -> Result<&Systemd>`:从 `&dyn ServiceManager` 向下转型,
  调用方必须先通过 `probe` 保证可用。

### 定义存储与注册

Landscape 的定义原件位于 `<landscape-root>/service/landscape-router.service`，安装
状态中的路径和摘要、事务注册快照均使用该原件。systemd 在读取初始启动依赖时必须
能够访问 unit 内容，因此供 PID 1 读取的副本位于系统盘：

```text
/etc/systemd/lkit-managed/<sha256-of-absolute-origin>/landscape-router.service
```

目录名是原件绝对路径字符串的 SHA-256。副本目录位于 systemd unit 搜索路径之外，
避免同名链接被解析为自指别名；`/etc/systemd/system/landscape-router.service` 指向该
副本。副本使用带版本的头部保存逻辑原件路径和原件正文摘要，并保留受管定义
内容及安装根的挂载要求；读取时验证副本路径与原件路径的对应关系、真实目录类型、
头部、摘要、内容和挂载要求。注册位置的普通文件、未知副本或无法验证归属的链接
均视为归属冲突，不覆盖或删除。原件仍是唯一的编辑和状态摘要依据，副本不是另一份
可独立修改的配置。

后端对已验证的副本返回逻辑原件路径，工作流及事务不依赖副本的物理位置。已知属于
同一原件的直接链接可迁移到副本；迁移前检查持久和运行时 `*.wants`／`*.requires`
成员链接的归属，再保留成员关系并更新目标。不改变 enabled、active，不为原本禁用
的服务创建启动成员。未知或外部成员冲突时在修改前拒绝。

`refresh_definition(service, origin)` 同步指定原件的启动副本后执行 daemon-reload；
`definition_needs_refresh` 同时检查原件内容与注册布局，原件字节未变也可能需要同步。
普通 `refresh()` 只重新加载管理器，不发布其他服务的原件修改。回滚恢复逻辑原件及
注册时同步对应副本；注销只删除属于本原件的已验证副本，即使原件或注册链接已被
清理也能幂等收尾。lkit daemon 的原件仍为 `/usr/local/lib/lkit/lkit.service`，它不使用
Landscape 的启动副本目录。

`is_enabled` 仅将 systemd 的 `enabled`、`enabled-runtime` 视为启用。`linked`、
`linked-runtime` 和 `alias` 表示可定位的 unit 身份，不表示已配置开机启动。

### 定义渲染

`render_definition` 按 `ManagedService` 渲染:

- `LandscapeRouter`:`<landscape-root>/current/landscape-webserver --config-dir
  <landscape-root>/data --web <landscape-root>/current/static`,含
  `LimitMEMLOCK=infinity`、`RequiresMountsFor="<canonical-landscape-root>"` 和
  `ExecStartPre=/usr/local/bin/lkit network startup-check`；启动副本可在安装根尚未挂载时
  被 PID 1 读取，服务启动须等待该安装根所需的挂载；
- `LkitDaemon`:`/usr/local/bin/lkit daemon --supervise`(lkit 全局二进制,unit 原件位于
  `/usr/local/lib/lkit/lkit.service`,不复制到任何安装根),
  含 `KillMode=process`——停服时只向主进程发信号,daemon 能完成进行中的委托
  请求,不会通过 cgroup 信号杀死执行子进程(停 lkit.service 自身即停服场景)。

`validate_definition` 校验 `ExecStart` 恰为对应受管命令、`User=root`、
`Restart=always`、`WantedBy=multi-user.target`(Landscape 额外要求 MEMLOCK,
daemon 额外要求 `KillMode=process`),且不含凭据内容。

## OpenRC 后端(简单实现)

`service/openrc.rs` 的 `Openrc` 结构体。服务名与 systemd 一致
(`landscape-router.service` / `lkit.service`):

- 注册:`/etc/init.d/<name>` 是指向原件的软链接;Landscape 原件位于
  `<landscape-root>/service/<name>`,lkit daemon 原件位于全局
  `/usr/local/lib/lkit/<name>`(与原 unit 文件相同的所有权冲突语义);
- 生命周期:`rc-service <name> {start|stop|restart|status}`;`status` 退出码
  3 表示停止,其他非零视为错误;
- 启用:`rc-update add|del <name> default`,`rc-update show` 解析启用表;
- 定义:`#!/sbin/openrc-run` 可执行脚本,`command=`/`command_args=` 指向受管可执行文件,
  校验 command 与 `command_user="root"` 且不含凭据；daemon 使用 pidfile 与有界停止等待，
  stdout/stderr 写入 `0600` 的 `/var/log/lkit.log`；
- `main_pid`:扫描 `/proc/*/cmdline`(OpenRC 无 MainPID 概念),轮询等待
  服务进程出现;
- `refresh` 为 no-op。

Landscape 脚本通过 `use lkit.service` 与 `after lkit.service` 声明对 daemon 的
弱依赖和启动顺序；Cgroup 则通过 `need cgroups` 声明硬依赖，使 Landscape 排在该服务
准备成功后启动。lkit daemon 本身不依赖 cgroups，环境未准备好时仍可运行以便修复。

控制台仅在识别到当前 OpenRC 环境且具备可执行的 cgroups/rc-service/rc-update 时提供
用户确认后的准备：缺少挂载时启动 cgroups，核实可写统一层级和 CPU controller，
再强制注册 `cgroups` 到 `boot` 并验证。已有 v1、混合、只读或冲突挂载不自动转换；
持久化失败不报告成功，也不卸载当前层级。不编辑内核启动参数或直接写资源限额，
系统服务按已有 OpenRC 配置运行。检查域与其他后端的展示见 [check](../check.md)。

## sysvinit 后端(简单实现)

`service/sysvinit.rs` 的 `Sysvinit` 结构体。服务名同样带 `.service` 后缀:

- 注册:`/etc/init.d/<name>` 是指向原件的软链接;
- 生命周期:直接以 `sh <script> {start|stop|restart}` 执行 LSB init 脚本
  (定义原件是受管文本文件,不要求可执行位);
- 启用:`update-rc.d enable|disable <name>`,`is_enabled` 扫描 `/etc/rc?.d`
  各运行级目录的 `S*<name>` 链接;
- 定义:`#!/bin/sh` LSB 脚本,内含 `start-stop-daemon` 调用；Landscape 通过
  `Should-Start: lkit` / `Should-Stop: lkit` 声明相对 daemon 的可选启动/停止顺序；
- `main_pid` 同 OpenRC 的 `/proc` 扫描。

## 后端接入清单

1. 实现 `ServiceManager`(生命周期、定义渲染/校验、注册、`main_pid`);
2. `ServiceManagerKind` 增加变体(serde lowercase),加入
   `ServiceManagerKind::supported()`,更新 `host_manager` 探测顺序与
   `docs/service/runtime-and-health.md` 的可用性判定;
3. 状态 schema 演进处理:旧 lkit 读新状态按未知枚举值报损坏并提示;
4. 按需新增测试 fixture(镜像 `lkit-test-init` 的多角色配置/call_log/state 模式,
   或 `lkit-test-systemctl`)。

## 宿主网络服务与启动检查

网络接管通过 `service::host_network` 调用各后端的服务状态与恢复操作。systemd 保存
原 enable/mask 状态；OpenRC 保存各 runlevel 链接；SysVinit 保存 S/K 链接名称、顺序
及目标。恢复按原运行状态启动或停止服务。

三个后端渲染的 Landscape 启动入口均执行 `/usr/local/bin/lkit network startup-check`。
服务启动检查与 daemon 进程存活依赖分开：已确认的 Landscape 不因 daemon 重启而停服。
详细契约见[daemon 网络恢复](../network/daemon-recovery.md)。
