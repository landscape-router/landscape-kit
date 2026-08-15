# 服务管理器抽象

`lkit` 通过 `ServiceManager` trait 抽象主流发行版 init 系统的服务操作。当前唯一
实现后端是 systemd;OpenRC、runit、sysvinit 等后端按需接入。

## 设计原则

- 契约只暴露 lkit 对服务的操作需求,不绑定任何具体 init 系统的概念;
- systemd 特有的概念(unit 名、MainPID 查询、daemon-reload、mask、注册软链接细节)
  全部留在后端内部,不进 trait;
- 接入新后端时以真实操作驱动契约演进,不做投机抽象;
- 后端必须满足 `Send + Sync`,可放入 `InstallRuntime.service_manager` 的
  `Box<dyn ServiceManager>`。

## trait 契约

```rust
trait ServiceManager: Send + Sync {
    fn kind(&self) -> ServiceManagerKind;                 // systemd(未来 openrc/runit/...)
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
  `service.manager`)与事务文件(`systemd_before`)中,当前只有 `systemd`。
  新增后端时增加变体并处理状态 schema 演进;
- `ManagedService`:lkit 需要托管的服务身份,当前有 `LandscapeRouter` 与
  `LkitDaemon`(lkit 常驻服务,Phase B 起使用);
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

- `LandscapeRouter`:`<root>/current/landscape-webserver --config-dir <root>/data
  --web <root>/current/static`,含 `LimitMEMLOCK=infinity`;
- `LkitDaemon`:`<root>/service/lkit daemon --config-dir <root>/data`
  (lkit 二进制复制到 `<root>/service/lkit`,与网络接管恢复二进制同目录约定)。

`validate_definition` 校验 `ExecStart` 恰为对应受管命令、`User=root`、
`Restart=always`、`WantedBy=multi-user.target`(Landscape 额外要求 MEMLOCK),
且不含凭据内容。

## 后端接入清单

1. 实现 `ServiceManager`(生命周期、定义渲染/校验、注册、`main_pid`);
2. `ServiceManagerKind` 增加变体(serde lowercase),更新 `select_manager` 的
   `Auto` 探测顺序与 `docs/service/runtime-and-health.md` 的可用性判定;
3. 状态 schema 演进处理:旧 lkit 读新状态按未知枚举值报损坏并提示;
4. 按需新增测试 fixture(镜像 `lkit-test-systemctl` 的配置/call_log/state 模式)。
