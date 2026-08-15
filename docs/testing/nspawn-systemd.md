# systemd-nspawn 兼容性 Smoke Test

## 目标与边界

`scripts/test-nspawn-systemd.sh` 启动最小 Debian rootfs，以真实 systemd 作为 PID 1。
它是低频兼容性 smoke test，只抽样验证 Docker fake 无法证明的 systemd 契约：

- `/bin/systemctl` 能连接 manager；
- system unit 能注册、enable、start 和 stop；
- 服务启动后真实 manager 报告非零 MainPID；
- lkit 常驻 daemon 由真实 systemd 托管，unit 含 `KillMode=process`。

测试使用 test-support 运行时跳过完整宿主 preflight，但配置
`execution: daemon` 并使用真实 `/bin/systemctl`、`/etc/systemd/system` 和
`/run/systemd/system`。这不是功能场景矩阵的第二份实现，不负责重新证明首次安装、
切换、修复或回滚的业务正确性，也不验证宿主 BPF/内核能力。

`systemd --user` 不能替代这一层：lkit 管理的是 root system unit、低位端口和系统 unit
目录。完整宿主 preflight 则仍由实际受支持主机或 VM 验收。

## 当前节点不处理卸载

卸载（`lkit uninstall`）场景当前不在此测试中验证：委托的 uninstall 需要交互确认，
而 daemon 执行子进程没有 controlling terminal（`cannot open /dev/tty; interactive
confirmation is not possible`），确认委托机制（如前端确认后以 `--console-confirmed`
注入）尚未实现。以下场景待文档明确需求后再补充：

- 委托 uninstall 的完整执行与提交；
- 前端会话被杀后 daemon 子进程组独立完成卸载；
- systemd unit 注册链接所有权冲突的失败路径（不停止服务、不删除外部文件）；
- 事务全部终结且卸载事务 `committed` 的收尾断言。

## 执行策略

该测试低频、手动或在 systemd 集成契约变化时运行，适合发现 unit 格式、真实 manager
调用和 worker 生命周期的兼容性回归。它不作为每个 PR 或普通发布的必需门禁，也不要求
为每个业务场景建立真实 systemd 版本。安装与生命周期的发布判断以 Rust fixture E2E
和 Docker 功能 E2E 为主，避免 nspawn 的 rootfs 下载、PID 1 启动和宿主差异阻碍发布。

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

测试先注册并启动受管的 landscape-router unit，再以 `lkit self-service install` 部署
常驻 daemon，断言 unit 注册、启停、MainPID 与 `KillMode=process`；最后停止并重新
启动两个受管服务，确认真实 manager 的 stop/start 契约。卸载与所有权冲突场景见
「当前节点不处理卸载」。
