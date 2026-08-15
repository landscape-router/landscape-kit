# 首次安装场景

## INS-01

**从 HTTP/RustFS 仓库安装显式版本**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[首次安装场景](../lifecycle.md#首次安装)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

## INS-02

**不指定版本时解析 stable 并安装 latest**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[latest 场景](../extended.md#s7-latest-通道安装)

## INS-03

**从默认 GitHub provider 完成首次安装**

- 测试层：Rust repository 测试
- 状态：`部分覆盖`
- 说明：provider 与下载逻辑分别覆盖，缺少完整 CLI 首装链路；见 [`lkit install`](../../../commands/install.md)。

## INS-04

**首次安装后初始化、启动并验证服务**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[首次安装断言](../lifecycle.md#首次安装)

## INS-05

**systemd 不可用时安装明确失败，不写任何文件**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：`first_install_fails_without_available_systemd`
  （crates/lkit-cli/src/workflows/install/first_install_tests.rs）；无 systemd 平台为
  unsupported，见[首次安装语义](../../../commands/install.md)。

## INS-07

**x86_64 和 aarch64 选择并启动各自发布资产**

- 测试层：双架构 Docker E2E
- 状态：`已覆盖`
- 证据：[Docker CI 边界](../../docker-e2e.md#本地运行)

## INS-08

**密码、初始化文件和 API token 满足输入校验与文件权限要求**

- 测试层：Rust 单元、CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[初始化与凭据](../../../interaction/initialization-and-credentials.md)、[首次安装场景](../lifecycle.md#首次安装)
- 边界：Ratatui 掩码、确认和 Debug 脱敏由控制台单元测试覆盖；端到端保密扫描单列在
  `SEC-02`。

## INS-09

**下载、压缩包校验或启动健康检查失败时清理首次安装现场**

- 测试层：Rust workflow、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[首次安装失败](../../../workflows/lifecycle.md#首次安装失败)、[失败清理 E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：Rust workflow 分别注入资产 404、损坏后端 zstd 和非法 static zip，并断言发布
  临时目录、目标 release、current、初始化文件和成功 state 均不存在且事务终结；CLI
  fixture E2E 另行验证启动健康检查失败后的服务、unit 与 resolv.conf 恢复。

## INS-10

**未知非空目录、危险软链接、已有不可信 release 或 foreign unit 阻断安装**

- 测试层：Rust 单元与 workflow
- 状态：`已覆盖`
- 证据：[验收标准](../../../acceptance.md#安装状态与路径)

## INS-11

**可复用的可信 release 不重复下载或覆盖**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[下载与发布目录](../../../repository.md#下载与发布目录)、[`release/artifacts.rs`](../../../../crates/lkit-cli/src/release/artifacts.rs)、[Docker E2E S14](../../../docker-e2e.md#场景)
- 说明：`build_release` 在下载前校验已有 `releases/<version>` 目录（真实目录非符号链接、后端二进制与 `static/index.html` 齐全、`static.zip` 摘要与 manifest 一致、Identity 编码时二进制摘要一致），可信则直接复用并跳过下载；不可信或残缺目录阻断且不修改。单元测试覆盖复用、摘要漂移、符号链接与残缺目录阻断，switch 集成测试覆盖回滚残留目录复用后不再请求目标资产，Docker E2E S14 覆盖失败切换回滚后残留目录复用的完整 CLI 落盘路径。

## INS-12

**首次安装在提交前中断，下次调用清理未提交现场**

- 测试层：Rust 事务测试
- 状态：`已覆盖`
- 证据：[事务恢复](../../../deployment/transactions-and-recovery.md)

## INS-13

**交互安装显示有序提示和下载进度，非交互安装保持纯文本输出**

- 测试层：Rust Ratatui TestBackend、PTY 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[安装输出规则](../../../commands/install.md)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：Ratatui 视图通过内存 TestBackend 验证，不要求测试进程连接真实终端；CLI fixture
  使用 `--password-file` 和捕获输出运行，并断言 stdout/stderr 不含 ANSI 转义序列。

## INS-14

**显式非交互参数禁止 TTY 提示和动态输出**

- 测试层：Clap 单元、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[安装输出规则](../../../commands/install.md)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：`--non-interactive` 在子命令前后均可解析；完整安装使用密码文件并断言输出不含
  ANSI 控制序列。额外 PTY 场景证明终端可用时该参数仍禁止密码提示并要求密码文件。

## INS-15

**Ctrl+C 恢复终端并取消当前安装**

- 测试层：PTY CLI fixture E2E
- 状态：`部分覆盖`
- 证据：[输出与退出码](../../../commands/output-and-exit-codes.md)、[事务托管](../../../deployment/transactions-and-recovery.md#systemd-托管操作)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 缺口：真实 systemd smoke 尚未直接验证 Ctrl+C 会停止临时 operation unit。
- 说明：PTY 场景在密码回显关闭后发送 SIGINT，断言退出状态为 `130` 且 `ECHO` 已恢复。

## INS-16

**`config.toml` 是只读用户配置：安装与后续命令都不写入，来源按优先级解析**

- 测试层：CLI fixture E2E
- 状态：`已覆盖`
- 证据：[配置文件](../../../deployment/config.md)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：首次安装完成后断言 `install-state.json` 不包含 `repository` 字段且安装根目录顶层
  不存在 `config.toml`；预置有效配置（HTTP 来源）后不带 `--repository` 执行首次安装，
  安装使用该来源且配置字节保持不变；网络接管 confirm 前后均不创建配置文件。
- 缺口：缺省官方 GitHub 的首次安装只在配置缺失路径被隐含覆盖，未单独断言请求打到官方
  仓库（E2E 避免真实网络）。

## INS-17

**来源解析优先级与损坏配置的按需阻断**

- 测试层：CLI fixture E2E、单元测试
- 状态：`已覆盖`
- 证据：[配置文件](../../../deployment/config.md)、[CLI fixture E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：解析优先级为 显式 CLI > `config.toml` > 官方 GitHub。`--repository` 支持精确小写值
  `github`；显式来源完全绕过配置（损坏配置下 reconcile/repair 仍成功，且原文件字节不变，
   预设的配置来源服务器收不到请求）。损坏配置只阻断需要仓库的命令（switch/repair/update
   无显式来源时报错并提示修复或删除），普通 reconcile、check、restore、backup、
   service-manager 和 network 子命令不受影响；删除文件后命令恢复。同版本显式来源诊断
   成功或失败都不修改配置。
