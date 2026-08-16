# systemd-nspawn 兼容性 Smoke Test

## 目标与边界

`scripts/test-nspawn-systemd.sh` 启动最小 Debian rootfs，以真实 systemd 作为 PID 1。
它是低频兼容性 smoke test，只抽样验证 Docker fake 无法证明的 systemd 契约：

- `/bin/systemctl` 能连接 manager；
- system unit 能注册、enable、start 和 stop；
- 服务启动后真实 manager 报告非零 MainPID；
- lkit 常驻 daemon 由真实 systemd 托管，unit 含 `KillMode=process`；
- daemon 委托执行器（`daemon_worker`）在真实 systemd 环境下的能力。

测试使用 test-support 运行时跳过完整宿主 preflight，但配置
`execution: daemon` 并使用真实 `/bin/systemctl`、`/etc/systemd/system` 和
`/run/systemd/system`。这不是功能场景矩阵的第二份实现，不负责重新证明首次安装、
切换、修复或回滚的业务正确性，也不验证宿主 BPF/内核能力。

`systemd --user` 不能替代这一层：lkit 管理的是 root system unit、低位端口和系统 unit
目录。完整宿主 preflight 则仍由实际受支持主机或 VM 验收。

## worker 委托能力场景

每个场景基于"卸载一次安装"展开：`lkit uninstall` 是委托命令，CLI 写请求到
`/run/lkit/operations`，daemon（真实 systemd 服务）认领后以子进程
（`--internal-daemon-worker`）执行真实 systemd 操作并写回结果，CLI 转发输出并
回收退出码。场景之间通过 `restore_scene` 恢复可卸载现场：

- **S-1 委托提交与结果回收**：CLI 全程等待委托的 uninstall 提交，断言状态删除、
  注册链接移除、`current` 移除、服务停止、保护 `.lkb` 保留、daemon 存活；
- **S-2 前端断开**：请求写入后 SIGKILL 前端进程，daemon 脱离会话独立完成卸载；
- **S-3 Ctrl+C 取消**：前端 SIGINT → CLI 返回 `130` 并写 cancel 文件；daemon 以
  SIGTERM 终止子进程组，下个周期前向完成中断的卸载（恢复语义）；
- **S-4 daemon 未运行**：`systemctl stop lkit.service` 后委托请求必须拒绝
  （退出码 `2` + "daemon is not running"），不卡住；
- **S-5 语言转发**：CLI 的 `LKIT_LANG=zh` 进入委托请求，worker 子进程用同一语言
  输出（中文缺失安装文案）。

尚未纳入 smoke 抽样：systemd unit 注册链接所有权冲突的失败路径（不停止服务、
不删除外部文件）。

## 执行策略

该测试低频、手动或在 systemd 集成契约变化时运行，适合发现 unit 格式、真实 manager
调用和 daemon 委托执行器（`daemon_worker` 子进程）生命周期的兼容性回归。它不作为
每个 PR 或普通发布的必需门禁，也不要求为每个业务场景建立真实 systemd 版本。安装与
生命周期的发布判断以 Rust fixture E2E 和 Docker 功能 E2E 为主，避免 nspawn 的
rootfs 下载、PID 1 启动和宿主差异阻碍发布。

## 运行

仅支持 Linux x86_64，需要 root、`systemd-nspawn`、`machinectl`、`systemd-run` 和
`mmdebstrap`：

```sh
cargo build --locked --release --features test-support -p lkit-cli --bin lkit
cargo build --locked --release -p lkit-test-fixture --bin landscape-webserver
sudo env LKIT_NSPAWN_PREBUILT_DIR="$PWD/target/release" \
  scripts/test-nspawn-systemd.sh
```

未设置 `LKIT_NSPAWN_PREBUILT_DIR` 时脚本也可自行构建，但 root 环境必须能直接使用项目
要求的 Rust toolchain。

脚本创建临时 trixie rootfs 和 private network namespace，结束时终止 machine 并删除
rootfs。CI 当前每周及手动运行；普通 PR 和普通发布不承担 rootfs 下载和 boot 成本。

测试先注册并启动受管的 landscape-router unit，再以 `lkit self install` 部署
常驻 daemon，断言 unit 注册、启停、MainPID 与 `KillMode=process`；随后按
「worker 委托能力场景」验证 daemon 委托执行器的提交、断开、取消、拒绝与语言转发
能力；最后停止并重新启动两个受管服务，确认真实 manager 的 stop/start 契约。
