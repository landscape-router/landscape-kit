# lkit 自身生命周期场景

`lkit self` 管理 lkit 自身(CLI 与全局常驻 daemon),与 landscape 安装解耦。

## SS-01

**`self install` 注册全局 daemon 监管入口，启用并启动**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 证据：[`lkit self`](../../../commands/self.md)
- 说明：断言全局 unit 原件 `/usr/local/lib/lkit/lkit.service` 的 `ExecStart` 指向
  `/usr/local/bin/lkit daemon --supervise`、fake systemctl 真实拉起监管进程与 daemon（`is-active`/
  `is-enabled`/`main.pid` 存活、daemon 写入自身 pidfile 到 `/root/.lkit/run/lkit.pid`）。
- 说明：`self install` 不接受安装根参数，服务定义使用全局 CLI 二进制。

## SS-02

**`self remove` 停止注销并清理全局 unit 原件，幂等可重复**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 证据：[`lkit self`](../../../commands/self.md)、[self fixture](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)
- 说明：断言 daemon 进程退出、注册链接与全局 unit 原件删除、unit 状态恢复 inactive；
  不删除 `/usr/local/bin/lkit`，不修改 lkit 地盘元数据。注册冲突时拒绝停止、禁用或删除外部服务，
  保留原件；修正注册后可重试，重复移除成功。禁用与 daemon-reload 故障注入验证
  非零退出、原件保留，以及注册链接已删除后的刷新重试。

## SS-03

**daemon 收到 SIGTERM/SIGINT 清理 pidfile 并干净退出**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`，经 `remove` 的 stop 路径）
- 状态：`已覆盖`
- 说明：fake systemctl `stop` 发送 SIGTERM，`remove` 后断言进程退出。

## SS-04

**`self install` 在服务后端或受管 CLI 不可用时拒绝修改**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 证据：[self fixture](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[后端准入 fixture](../../../../lkit-cli/tests/install_fixture_e2e/manager_backends.rs)
- 说明：所选后端不可用，或缺少受管 CLI 时，退出码 `2` 且不写服务原件、不注册、不启动 daemon。OpenRC／SysVinit 准入测试隔离全局 CLI 路径；这些拒绝场景不代表后端生命周期覆盖。

## SS-05

**`self upgrade` 下载校验并原子替换 `/usr/local/bin/lkit`，daemon 注册且运行时 restart**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 证据：[`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[`lkit self`](../../../commands/self.md#upgrade)
- 说明：本地 Release fixture 覆盖下载→SHA256SUMS 校验→自检→原子替换；fake systemd 真实 stop/start unit，断言新 daemon 从替换后的文件启动。
## SS-06

**`self upgrade` 与目标版本相同返回 `0`，不修改任何文件**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 说明：同版本只读取 Release API 元数据，不请求 checksum/asset，不修改目标文件。
- 证据：[`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[`lkit self`](../../../commands/self.md#upgrade)
## SS-07

**`self upgrade` 下载/校验/自检/替换失败保留原二进制**

- 测试层：Fixture E2E + Rust 单元
- 状态：`部分覆盖`
- 证据：[`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[`lkit self`](../../../commands/self.md#upgrade)
- 说明：Fixture E2E 覆盖 SHA256 不匹配和版本自检失败时原二进制保持可用；rename 失败仍由 `install_staged_binary` 的单元测试补充，尚未接入完整 fixture 场景。
## SS-08

**`self upgrade` daemon 未注册时仅更新 CLI 并提示 `self install`**

- 测试层：Fixture E2E（`install_fixture_e2e::self_cmd`）
- 状态：`已覆盖`
- 说明：daemon 未注册（`refresh_daemon` 的 Missing 分支）时仅更新 CLI、输出 `self install` 提示且不执行 restart。
- 证据：[`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[`lkit self`](../../../commands/self.md#upgrade)
## SS-09

**daemon 全局唯一：地盘实例锁拒绝第二个 daemon**

- 测试层：Rust workflow、Fixture E2E（待补充）
- 状态：`部分覆盖`
- 证据：[`lkit self`](../../../commands/self.md)、[安装布局与状态](../../../deployment/layout-and-state.md)
- 说明：daemon 以地盘实例锁保证单例，陈旧 PID 文件不阻止恢复；重复 `self install`
  刷新原服务，不产生第二个并行 daemon。
## SS-10

**`self install` 设置私有发现密钥：daemon 启动前写入 `[flare]` 段**

- 测试层：Rust 单元、Fixture E2E
- 状态：`已覆盖`
- 证据：[`lkit self`](../../../commands/self.md#install)、[`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[flare 协议](../../../flare/protocol.md)
- 说明：`lkit self install --flare-psk-file`（root-only 私密文件）在 daemon 启动前把
  psk 写入地盘 `config.toml` 的 `[flare]` 段（0600，保留既有字段）；未提供时保留
  已有设置，新装默认公开发现。单元测试覆盖从文件设置密钥、保留现有配置及拒绝过短密钥。

## SS-12

**自动升级不下载同版本或更低版本，显式版本仍允许选择旧版**

- 测试层：Rust 单元、临时回环 Release API、Fixture E2E（仅 CI／容器）
- 状态：`已覆盖`
- 证据：[版本决策与请求断言](../../../../lkit-cli/src/commands/self_upgrade_tests.rs)、[命令级不下载／不替换断言](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、[升级规格](../../../commands/self.md#upgrade)
- 说明：当前 0.7.1-rc4、默认正式渠道返回 0.6.2 时只请求 Release 元数据，不请求校验清单或二进制。覆盖更高正式版、候选序号、rcN/rc.N、build metadata 与显式旧版选择；暂存自检执行失败的错误包含路径和加载器提示，单元测试不执行真实下载文件；CI fixture 使用缺失解释器的资产验证执行失败时保留原二进制并清理暂存文件。

## SS-11

**`self upgrade` 默认正式版，显式 opt-in 自动选择 RC 等预发布版本**

- 测试层：Rust 单元（临时回环 HTTP）+ Fixture E2E
- 状态：`已覆盖`
- 证据：[`self_upgrade_tests.rs`](../../../../lkit-cli/src/commands/self_upgrade_tests.rs)、
  [`self_cmd.rs`](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)、
  [`lkit self upgrade`](../../../commands/self.md#upgrade)、
  [Fixture E2E 成功记录](https://github.com/landscape-router/landscape-kit/actions/runs/41378532454)
- 说明：默认只请求 `releases/latest`，拒绝预发布标记和误标为正式版的 RC tag；
  `--include-prerelease` 仅适用于 `self upgrade` 及其 `self update` 别名。
  允许预发布时读取分页列表，排除 draft/非法 tag，按版本优先级选择而非按创建时间；
  覆盖 `rc2`/`rc10`/`rc.11`、正式版优先、后续页更高版本、空列表、API 错误、分页失败
  与 100 页上限，不使用部分结果。显式 `--version` 优先于自动发现，候选 pin 无需额外
  选项；draft 和不匹配的 API tag 被拒绝。单元测试不启动子进程，只用临时回环端口。
  Fixture E2E 定义稳定/RC 自动发现后的完整校验、自检和替换链路，以及无 stable 时
  默认失败、原二进制和下载请求保持不变。

## SS-13

**自身服务注册冲突拒绝修改，重复安装失败恢复原服务**

- 测试层：Rust 单元（临时目录、注入服务管理器）、Fixture E2E（CI）
- 状态：`已覆盖`
- 证据：[self 管理器与文件断言](../../../../lkit-cli/src/commands/self.rs)、
  [OpenRC 注册](../../../../lkit-cli/src/service/openrc.rs)、[SysVinit 注册](../../../../lkit-cli/src/service/sysvinit.rs)、
  [self fixture](../../../../lkit-cli/tests/install_fixture_e2e/self_cmd.rs)。
- 说明：外部文件、外部注册链接、定义原件符号链接均在修改前拒绝；升级刷新不得重启
  外部服务。重复安装的 restart、启动或就绪验证失败恢复旧定义、权限和启用/运行状态；
  指向受管路径的悬空注册可修复，失败保留新定义以便重试；首次安装失败清理新建服务，
  清理失败保留原件。OpenRC/SysVinit 注册冲突测试只读写
  临时目录；服务动作单测使用注入管理器，真实子进程场景由 CI fixture 执行。
