# Docker 功能 E2E

## 目标

该测试验证发布、安装、切换、修复、备份、回滚、service manager 迁移、中断恢复和
reconcile 的完整业务状态机。Docker 的职责是提供干净、可复现的 Debian 文件系统和
RustFS 网络，不是模拟一台完整启动的 Linux 主机。

```text
Docker Compose network
├── rustfs：S3 API 与公开 HTTP release repository
└── e2e：测试版 lkit + fake systemctl + fake Landscape
```

runner 是普通容器，不使用：

- `privileged: true`；
- systemd PID 1；
- private cgroup namespace 或宿主 cgroup 挂载；
- `/boot`、`/run`、`/etc/resolv.conf` 宿主能力模拟。

## 被测边界

Docker 中的 `lkit` 以 `test-support` 构建，所有管理命令显式传入同一个测试运行时：

```json
{
  "preflight": "skip",
  "execution": "inline",
  "systemd": {
    "systemctl": "/usr/local/bin/lkit-test-systemctl",
    "pid1_is_systemd": true
  }
}
```

`preflight: skip` 只关闭 root/BPF/内核/宿主依赖等完整环境审计。命令仍选择
`--service-manager systemd`，仍创建和校验 unit、调用 systemctl 协议、停止和启动真实
fixture 进程、检查 MainPID/端口/API，并按事务回滚。

`execution: inline` 避免要求 fake systemctl 再模拟 lkit 自己的临时 worker unit。
生产运行时始终执行完整 preflight；会改变 systemd 或 Landscape 运行态的生产命令由
systemd worker 托管，且生产二进制不提供 `--test-runtime`。

fake systemctl 的配置不固定 `landscape_config`，因此每次 start 都由 unit 的 `--web`
路径读取当前 release 的 `static/lkit-fixture.json`。调用序列追加到 root-only JSONL
日志，便于断言 service-manager 协议。

该层是安装与生命周期功能正确性的主要 E2E 证据。资产下载与摘要校验、安装目录和状态
写入、事务提交与恢复、fixture 进程启动、MainPID/端口/API 检查都真实执行；只有外部
service manager 接口由 fake systemctl 隔离。功能场景不要求再在真实 systemd 下逐项
重复，nspawn 仅保留低频兼容性 smoke 覆盖。

## Release 生成

镜像只编译一次 `landscape-webserver`。`lkit-fixture-release --stamp-version` 在 fixture
ELF 尾部加入版本标记；Linux 可执行加载不受影响，各发布版本仍具有不同 SHA，能够
验证版本身份与 repair。fixture 场景继续由各 release 的 static 配置声明。

慢启动场景使用测试运行时的 4 秒启动超时和 10 秒 ready delay，不再为验证同一状态机
等待生产的 180 秒。

## 场景

`run-scenarios.sh` 保留 S1-S4 与 S6-S9，并新增 S10-S13：

- 二进制/static repair；
- 导出失败、启动即退、稳定期退出、慢启动回滚；
- 已停止服务的默认拒绝和 `--allow-no-backup`；
- none/systemd 迁移、latest、事务恢复与 reconcile；
- 手工备份与同版本恢复（S10）；
- restore 激活失败自动回滚（退出码 `5`，S11）；
- restore 中断后的 phase 恢复（S12）；
- systemd 跨版本 restore（S13）；
- 失败切换回滚后残留 release 目录的可信复用（S14）。

S1-S8、S11-S13 的 repair/switch/update 需要仓库来源。`install` 自 0.1.4 起不再持久化
来源，场景在各安装根完成首次安装后写入 `config.toml`（`[repository]` HTTP 来源），
顺带验证配置驱动的来源解析路径。

宿主 `lkit check` 不属于 Docker 功能矩阵；它验证宿主内核和依赖能力，应在实际支持
主机或完整 VM 验收中执行。

## 本地运行

```sh
scripts/test-docker-lifecycle.sh
```

脚本要求 Docker 与 Compose v2。无论成功或失败，都会删除容器、网络、RustFS volume
和临时结果目录。ARM 本地环境不自动使用 QEMU；CI 在原生 x86_64/aarch64 runner 上
执行同一场景。

每次运行生成唯一 RustFS bucket 和对应 repository URL。S1-S4、S6-S9 在一次运行内
共享 release 历史，但不会复用上次中断或并行运行留下的发布对象；入口还会先清理同一
Compose 项目遗留的容器和 volume。

结果写入 `result.json`，stdout/stderr 同时写入 `scenario.log`。非交互 runner 直接返回
场景退出码。

RustFS 仍固定镜像 digest。HTTP 仓库继续通过容器内 `127.0.0.1` TCP 代理访问，以符合
lkit 对明文 HTTP 仅允许 loopback 的安全规则。
