# systemd-nspawn 兼容性 Smoke Test

## 目标与边界

`scripts/test-nspawn-systemd.sh` 启动最小 Debian rootfs，以真实 systemd 作为 PID 1。
它是低频兼容性 smoke test，只抽样验证 Docker fake 无法证明的 systemd 契约：

- `/bin/systemctl` 能连接 manager；
- system unit 能注册、enable、start、stop、disable 和 unregister；
- 服务启动后真实 manager 报告非零 MainPID；
- lkit 常驻 daemon 由真实 systemd 托管并处理委托请求；
- 杀掉等待结果的前端会话后，daemon 子进程组继续完成事务；
- systemd unit 所有权冲突返回失败，且不会留下失败状态。

测试使用 test-support 运行时跳过完整宿主 preflight，但配置
`execution: daemon` 并使用真实 `/bin/systemctl`、`/etc/systemd/system` 和
`/run/systemd/system`。这不是功能场景矩阵的第二份实现，不负责重新证明首次安装、
切换、修复或回滚的业务正确性，也不验证宿主 BPF/内核能力。

`systemd --user` 不能替代这一层：lkit 管理的是 root system unit、低位端口和系统 unit
目录。完整宿主 preflight 则仍由实际受支持主机或 VM 验收。

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

测试先执行完整的首次安装并注册、启动受管 unit。在事务进入 `verifying` 后杀掉
`machinectl shell` 前端，等待委托的 daemon 子进程组独立提交；随后执行卸载，断言服务
停止、注册链接移除、所有事务终结且没有残留运行状态。
最后制造一个 foreign unit 所有权冲突，确认注册在创建事务前失败，且不留失败状态。
