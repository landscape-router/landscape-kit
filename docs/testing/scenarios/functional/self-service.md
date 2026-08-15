# lkit 自服务场景

## SS-01

**`self-service install` 复制二进制、渲染定义并注册启用启动 daemon**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 证据：[`lkit self-service`](../../commands/self-service.md)
- 说明：断言 `<root>/service/lkit` 可执行、`service/lkit.service` 的 `ExecStart`
  指向 `--config-dir <root>/data`、fake systemctl 真实拉起 `lkit daemon`
  （`is-active`/`is-enabled`/`main.pid` 存活、daemon 写入自身 pidfile）。

## SS-02

**`self-service remove` 停止注销并清理二进制，幂等可重复**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 证据：[`lkit self-service`](../../commands/self-service.md)
- 说明：断言 daemon 进程退出、unit 原件与二进制删除、unit 状态恢复 inactive。

## SS-03

**daemon 收到 SIGTERM/SIGINT 清理 pidfile 并干净退出**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`，经 `remove` 的 stop 路径）
- 状态：`已覆盖`
- 说明：fake systemctl `stop` 发送 SIGTERM，`remove` 后断言进程退出。

## SS-04

**`self-service install` 拒绝 `--service-manager none` 或 systemd 不可用**

- 测试层：Fixture E2E（`install_fixture_e2e::self_service`）
- 状态：`已覆盖`
- 说明：`--service-manager none` 与「systemctl 不可用」均断言退出码 `2`
  且不写任何文件。
