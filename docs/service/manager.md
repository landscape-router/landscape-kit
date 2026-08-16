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
    fn is_active / active_state / start / stop / restart;
    fn stop_and_wait(service, wait_for_exit) -> Result<()>;   // 默认实现:stop + 轮询
    fn refresh(&self) -> Result<()>;                           // 默认 no-op(systemd 覆盖为 daemon-reload)

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
  `Conflict { file_type }`);
- `Registration` / `RegistrationKind`:序列化的事务前注册状态;
- `ServiceBefore`:受管服务事务前状态(注册 + enabled + active),失败回滚时恢复用。

## 事务前状态捕获

```rust
capture_before(manager: &dyn ServiceManager, service: ManagedService) -> Result<ServiceBefore>
```

通用实现:查询注册 + enabled + active。注册所有权冲突(`Conflict`)时阻断,
不能自动接管。

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

### 定义渲染

`render_definition` 按 `ManagedService` 渲染:

- `LandscapeRouter`:`<landscape-root>/current/landscape-webserver --config-dir
  <landscape-root>/data --web <landscape-root>/current/static`,含
  `LimitMEMLOCK=infinity`;
- `LkitDaemon`:`/usr/local/bin/lkit daemon`(lkit 全局二进制,unit 原件位于
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
- 定义:`#!/sbin/openrc-run` 脚本,`command=`/`command_args=` 指向受管可执行文件,
  校验 command 与 `command_user="root"` 且不含凭据;
- `main_pid`:扫描 `/proc/*/cmdline`(OpenRC 无 MainPID 概念),轮询等待
  服务进程出现;
- `refresh` 为 no-op。

未覆盖的 OpenRC 特性(runlevels 之外、service 依赖)留给真实操作驱动。

## sysvinit 后端(简单实现)

`service/sysvinit.rs` 的 `Sysvinit` 结构体。服务名同样带 `.service` 后缀:

- 注册:`/etc/init.d/<name>` 是指向原件的软链接;
- 生命周期:直接以 `sh <script> {start|stop|restart}` 执行 LSB init 脚本
  (定义原件是受管文本文件,不要求可执行位);
- 启用:`update-rc.d enable|disable <name>`,`is_enabled` 扫描 `/etc/rc?.d`
  各运行级目录的 `S*<name>` 链接;
- 定义:`#!/bin/sh` LSB 脚本,内含 `start-stop-daemon` 调用;
- `main_pid` 同 OpenRC 的 `/proc` 扫描。

## 后端接入清单

1. 实现 `ServiceManager`(生命周期、定义渲染/校验、注册、`main_pid`);
2. `ServiceManagerKind` 增加变体(serde lowercase),加入
   `ServiceManagerKind::supported()`,更新 `host_manager` 探测顺序与
   `docs/service/runtime-and-health.md` 的可用性判定;
3. 状态 schema 演进处理:旧 lkit 读新状态按未知枚举值报损坏并提示;
4. 按需新增测试 fixture(镜像 `lkit-test-init` 的多角色配置/call_log/state 模式,
   或 `lkit-test-systemctl`)。
