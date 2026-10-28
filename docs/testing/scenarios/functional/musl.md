# musl 跨架构轻量功能测试

CI 在 Linux x86_64 runner 上编译四种 musl 测试二进制：x86_64 原生执行，aarch64、
riscv64、loongarch64 分别通过 QEMU user-mode 执行。测试入口为
[`scripts/test-musl.sh`](../../../../scripts/test-musl.sh)，workflow 为
[`test-musl.yml`](../../../../.github/workflows/test-musl.yml)。

每个模块串行执行，只操作隔离临时目录；CLI 明确使用 `--bin lkit`，不启用
`test-support`，不编译运行 fixture 服务。每个筛选器必须实际通过至少一个测试，
零测试结果会使 CI 失败。测试日志作为 Actions artifact 保存 7 天。
测试 profile 使用 `opt-level=1`、保留调试断言；scrypt 环境开销覆盖设为 10，
显式指定开销的加密测试仍使用其自身参数。

## MUSL-01

**协议编码、加密校验、防重放和握手状态机在四种架构上运行一致**

- 测试层：musl 单元测试，原生/QEMU user-mode
- 状态：`已覆盖`
- 证据：[协议测试](../../../../landscape-terrain-proto/src/protocol/)、[CI](../../../../.github/workflows/test-musl.yml)
- 说明：运行 `landscape-terrain-proto --lib protocol::`，不打开 AF_PACKET 或真实网卡。

## MUSL-02

**配置解析、验证和写回在四种架构上运行一致**

- 测试层：musl 单元测试，原生/QEMU user-mode
- 状态：`已覆盖`
- 证据：[配置测试](../../../../lkit-cli/src/deployment/config.rs)、[测试入口](../../../../scripts/test-musl.sh)
- 说明：运行 `deployment::config::tests::`，固定地盘通过 `LKIT_TERRITORY` 重定向到临时目录。

## MUSL-03

**归档解压、安全路径和损坏输入校验在四种架构上运行一致**

- 测试层：musl 单元测试，原生/QEMU user-mode
- 状态：`已覆盖`
- 证据：[归档测试](../../../../lkit-cli/src/release/repository/archive.rs)、[测试入口](../../../../scripts/test-musl.sh)
- 说明：运行 `release::repository::archive::tests::`，覆盖 zstd、ZIP 和路径逃逸拒绝。

## MUSL-04

**备份生成、读取、校验和解包在四种架构上运行一致**

- 测试层：musl 单元测试，原生/QEMU user-mode
- 状态：`已覆盖`
- 证据：[备份读取测试](../../../../lkit-cli/src/backup/lkb/read.rs)、[备份写入测试](../../../../lkit-cli/src/backup/lkb/write.rs)、[测试入口](../../../../scripts/test-musl.sh)
- 说明：分别运行 `backup::lkb::read::tests::` 和 `backup::lkb::write::tests::`，源数据与解包路径全部位于临时目录。

## MUSL-05

**真实架构内核、服务管理与网络恢复兼容性**

- 测试层：目标架构虚拟机或实机
- 状态：`待补充`
- 证据：[CI 范围](../../../../.github/workflows/test-musl.yml)、[systemd smoke](../systemd-smoke.md)
- 缺口：QEMU user-mode 使用宿主内核，不能证明目标架构 systemd、AF_PACKET、eBPF、daemon 部署和网络接管的实际兼容性；这些不属于轻量测试的覆盖范围。
