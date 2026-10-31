# 并发任务与线程命名

应用创建的 Tokio 任务使用原生 task name，专用系统线程与 Tokio runtime worker 使用
系统线程名。名称在普通、release 和 dist 构建中均启用，不依赖诊断 feature。

## 名称与创建入口

各 crate 的名称只集中定义在一个 `src/task_names.rs` 文件中：

- `lkit-cli`：daemon flare 服务、服务端连接与退出监听、控制台后台线程、镜像探测线程、runtime worker。
- `landscape-flare`：客户端任务、转发监听与连接、退出监听、runtime worker。
- `landscape-terrain-proto`：链路层接收线程。
- `lkit-test-fixture`：fixture 的 HTTP/HTTPS、DNS、退出定时任务与 runtime worker。
- `lkit-publish`：发布工具的 runtime worker。

`landscape-terrain-proto::concurrency` 提供共用的 `spawn_task`、`spawn_thread` 和
`runtime_thread_name_fn`。命名封装保留 `JoinHandle` 的返回值、取消和 join 语义；
任务或线程创建失败时 panic。fixture 直接使用 Tokio 的命名 builder 并传播创建错误。
第三方库内部创建的任务以及现有测试辅助线程不在应用命名覆盖范围内。

任务名采用稳定的点分层级，例如 `daemon.flare`、`flare.server.connection`；
同一类连接共用任务名，由任务 ID 区分实例。不要把 MAC、端口、令牌或凭据拼进名称。
系统线程名使用短字符串；Linux 可见名称最多为 15 字节。runtime worker 名称包含递增编号。

## 构建约束

Tokio 原生命名 builder 需要 `tracing` feature 与 `--cfg tokio_unstable`。
工作区依赖启用前者，`.cargo/config.toml` 的 `build.rustflags` 启用后者，所有构建
profile 使用同样的命名代码。

覆盖 `RUSTFLAGS`、`CARGO_ENCODED_RUSTFLAGS` 或目标平台 rustflags 时，必须保留
`--cfg tokio_unstable`；musl 构建和测试脚本在目标平台 flags 中显式携带该配置。
Docker 源码构建需要复制 `.cargo/config.toml`。在工作区外构建该协议库时也必须配置
这一 rustflag。Tokio 的该 API 不稳定，升级 Tokio 时需要重新检查兼容性。

## 查看名称

- `top -H -p <PID>`、htop 的线程视图、`/proc/<PID>/task/*/comm` 可查看系统线程名，
  例如 `lkit-rt-0`、`terrain-rx`。htop 可能需要开启自定义线程名显示。
- Tokio 任务不是系统线程，会在 worker 间调度，不能通过 htop 直接查看其名称或 CPU。
  原生任务名通过 Tokio 的 tracing 元数据暴露，需要连接兼容的 tracing subscriber
  或任务诊断工具才能采集；程序默认不安装 console subscriber，也不开放诊断端口。
