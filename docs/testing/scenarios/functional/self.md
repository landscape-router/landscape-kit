# lkit 自身生命周期场景

`lkit self` 管理 lkit 自身(CLI 与全局常驻 daemon),与 landscape 安装解耦。

## SS-01

**`self install` 注册全局 daemon：`ExecStart=/usr/local/bin/lkit daemon`，启用并启动**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 证据：[`lkit self`](../../../commands/self.md)
- 说明：断言全局 unit 原件 `/usr/local/lib/lkit/lkit.service` 的 `ExecStart` 指向
  `/usr/local/bin/lkit daemon`、fake systemctl 真实拉起 `lkit daemon`（`is-active`/
  `is-enabled`/`main.pid` 存活、daemon 写入自身 pidfile 到 `/root/.lkit/run/lkit.pid`）。
- 缺口：`--install-dir` 已从 `self install` 移除，原断言的 `<root>/service/lkit` 复制
  与 `--config-dir` 参数不再存在。

## SS-02

**`self remove` 停止注销并清理全局 unit 原件，幂等可重复**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 证据：[`lkit self`](../../../commands/self.md)
- 说明：断言 daemon 进程退出、注册链接与全局 unit 原件删除、unit 状态恢复 inactive；
  不删除 `/usr/local/bin/lkit`，不修改 lkit 地盘元数据。

## SS-03

**daemon 收到 SIGTERM/SIGINT 清理 pidfile 并干净退出**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`，经 `remove` 的 stop 路径）
- 状态：`已覆盖`
- 说明：fake systemctl `stop` 发送 SIGTERM，`remove` 后断言进程退出。

## SS-04

**`self install` 拒绝 systemd 不可用**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 说明：「systemctl 不可用」断言退出码 `2` 且不写任何文件。

## SS-05

**`self upgrade` 下载校验并原子替换 `/usr/local/bin/lkit`，daemon 注册且运行时 restart**

- 测试层：Fixture E2E（待补充）
- 状态：`待补充`
- 证据：[`lkit self`](../../../commands/self.md#upgrade)
- 说明：断言下载对应架构二进制与 `SHA256SUMS`、校验与 `--version` 自检通过后原子替换；
  daemon `is-active` 时执行 restart 并加载新二进制。

## SS-06

**`self upgrade` 与目标版本相同返回 `0`，不修改任何文件**

- 测试层：Fixture E2E（待补充）
- 状态：`待补充`
- 证据：[`lkit self`](../../../commands/self.md#upgrade)

## SS-07

**`self upgrade` 下载/校验/自检/替换失败保留原二进制**

- 测试层：Fixture E2E（待补充）
- 状态：`待补充`
- 证据：[`lkit self`](../../../commands/self.md#upgrade)
- 说明：SHA256 不匹配、自检失败或替换失败时原 `/usr/local/bin/lkit` 保持可用，返回 `1`。

## SS-08

**`self upgrade` daemon 未注册时仅更新 CLI 并提示 `self install`**

- 测试层：Fixture E2E（待补充）
- 状态：`待补充`
- 证据：[`lkit self`](../../../commands/self.md#upgrade)

## SS-09

**daemon 全局唯一：lkit 地盘 pidfile 存活实例存在时拒绝启动**

- 测试层：Rust workflow、Fixture E2E（待补充）
- 状态：`待补充`
- 证据：[`lkit self`](../../../commands/self.md)、[安装布局与状态](../../../deployment/layout-and-state.md)
- 说明：同一 pidfile 存活实例存在时 `self install`/daemon 启动拒绝并返回失败，不产生
  第二个 daemon。
