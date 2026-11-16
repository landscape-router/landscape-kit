# systemd-nspawn 兼容性 Smoke Test

## 目标与边界

`scripts/test-nspawn-systemd.sh` 启动最小 Debian rootfs，以真实 systemd 作为 PID 1。
它是低频兼容性 smoke test，只抽样验证 Docker fake 无法证明的 systemd 契约：

- `/bin/systemctl` 能连接 manager；
- system unit 能注册、enable、start 和 stop；
- 服务启动后真实 manager 报告非零 MainPID；
- lkit 常驻 daemon 由真实 systemd 托管，unit 含 `KillMode=process`；
- daemon 原生操作在真实 systemd 环境下的执行和恢复能力。

测试使用 test-support 运行时跳过完整宿主 preflight，但配置
`execution: daemon` 并使用真实 `/bin/systemctl`、`/etc/systemd/system` 和
`/run/systemd/system`。这不是功能场景矩阵的第二份实现，不负责重新证明首次安装、
切换、修复或回滚的业务正确性，也不验证宿主 BPF/内核能力。

`systemd --user` 不能替代这一层：lkit 管理的是 root system unit、低位端口和系统 unit
目录。完整宿主 preflight 则仍由实际受支持主机或 VM 验收。

## 原生操作场景

每个场景基于一次卸载验证 systemd 契约，CLI 提交结构化操作，daemon 直接执行，
操作记录保留在 `/root/.lkit/run/operations/`。场景之间恢复隔离安装现场。

- **S-1 提交与查询**：卸载成功后按 operation ID 查询提交结果，检查状态、注册、current
  已清理，保护备份保留，daemon 仍运行。
- **S-2 前端断开**：在停止阶段检查点杀死观察端，解除检查点后 daemon 完成原操作。
- **S-3 daemon 崩溃恢复**：运行中的取消请求被拒绝；杀死实际 daemon 后，监管进程
  自动重启它，恢复按原事务前向完成卸载，同一 operation ID 收敛为成功。
- **S-4 daemon 不可用**：停止 lkit 服务后提交在有限时间内失败，不入队、不改变安装。
- **S-4b 可执行文件替换**：daemon 运行时删除其二进制路径，使用独立 CLI 副本提交卸载，
  原 daemon PID 完成操作；随后恢复 CLI 文件。
- **S-5 操作语言**：`LKIT_LANG=zh` 的前端提交卸载，回执保存 `language: zh` 并输出中文
  结果，语言不通过子进程环境传播。

停止阶段检查点只编译于 test-support 构建，文件位于容器的 lkit 地盘，不影响生产路径。

## 执行策略

该测试低频、手动或在 systemd 集成契约变化时运行，适合发现 unit 格式、真实 manager
调用和 daemon 原生操作生命周期的兼容性回归。它不作为
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

脚本创建临时 trixie rootfs 和 private network namespace。结束时通过 `machinectl terminate`
请求停止 machine，等待本脚本启动的 nspawn 子进程退出并回收后，才删除 rootfs；不能将
异步终止请求返回当作容器已经退出。正常退出最多等待 10 秒，必要时向自有 nspawn 进程
发送 TERM 并再等 10 秒，最后 KILL 后最多再等 5 秒。若使用了 KILL，即使父进程已退出
也保留目录并报错，因为不能据此确认 guest namespace 已正常清理。进程仍未退出、
无法检查挂载，或临时树下仍存在挂载时同样保留目录，不强制卸载、不跨文件系统删除。
清理保留原测试失败退出码；测试成功但清理失败时仍报告失败。

[`scripts/test-nspawn-cleanup.sh`](../../scripts/test-nspawn-cleanup.sh) 使用 shell 函数替身
验证退出等待、信号升级、残留挂载保护、删除失败、启动前失败和危险参数拒绝；只读写
临时目录，不启动真实容器、不发送真实进程信号。CI 在系统集成测试前运行该单元测试及
ShellCheck。CI 当前每周及手动运行；普通 PR 和普通发布不承担 rootfs 下载和 boot 成本。

测试先注册并启动受管的 landscape-router unit，再以 `lkit self install` 部署
常驻 daemon，断言 unit 注册、启停、MainPID 与 `KillMode=process`；随后按
「原生操作场景」验证提交、断开、拒绝运行中取消、崩溃恢复、能力检查与操作语言
能力；最后停止并重新启动两个受管服务，确认真实 manager 的 stop/start 契约。
