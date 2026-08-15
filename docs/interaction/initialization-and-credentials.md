# 初始化与凭据

## 初始化配置与凭据

### 最小初始化接口

首次安装由 `lkit` 创建：

```text
<install-root>/data/landscape_init.toml
```

最小内容固定为：

```toml
version = "0.19.2"

[config.auth]
admin_user = "admin"
admin_pass = "<password>"
```

其中 `version` 必须等于目标 Landscape 版本。该最小格式是 Landscape 与 `lkit` 的稳定安装接口。

`landscape_init.toml` 是一次性初始化输入；`landscape.toml` 是 Landscape 生成和维护的持久运行配置。两者不是同一文件，也不是重命名关系。

首次启动成功后应存在：

- `data/landscape_init.lock`；
- `data/landscape.toml`。

目标版本不接受最小格式时，启动验证失败并进入回滚。

### 管理员凭据

- 默认用户名为 `admin`；
- `--admin-user` 可覆盖，拒绝空值和控制字符；
- Ratatui Install 面板直接收集并掩码显示密码和确认密码，不要求用户创建密码文件；
- 交互模式通过 `/dev/tty` 隐藏输入密码并要求二次确认；
- 无法打开 `/dev/tty` 时必须使用 `--password-file`；
- 不提供明文密码命令行参数；
- 不把密码写入日志、状态或事务文件。

控制台 inline 安装在内存中传递密码。委托安装使用
`/run/lkit/operations/<id>.credential` 作为 root-only `0600` 的短生命周期内部通道；该
文件不是用户输入接口；request JSON 只记录路径，不记录密码内容。daemon 与前端按
完成或取消路径清理。若委托命令
无法停止则保留现场，不能提前删除仍可能被运行中命令使用的凭据。

### 交互规则

密码和所有确认输入只通过 `/dev/tty` 读取，不读取标准输入，避免消费管道或重定向数据。
直接执行时提示写入 `/dev/tty`；systemd worker 执行时，提示与部署前 warning 使用同一条
stderr 转发通道，保证 warning 不会因两条输出通道竞争而插入密码提示之后。隐藏输入完成后
始终输出换行，使两次密码提示和后续消息各自占一行。无法打开 `/dev/tty` 时视为非交互模式。

公开安装说明因此推荐先通过管道安装 `lkit`，再直接从终端运行 `sudo lkit` 并从 Install
面板启动；脚本继续使用明确的 `lkit install ...` 子命令。
把下载脚本与 `install` 合并在同一条管道命令中时，调用环境仍可能没有可打开的
`/dev/tty`；此时不会回退到 stdin，而是要求 `--password-file`。

全局 `--non-interactive` 即使在 `/dev/tty` 可用时也禁止打开终端。自动化调用应显式使用
该参数并提供所需的 `--password-file`、`--accept-*` 等专用参数，避免伪终端、CI runner
或测试框架改变交互判断。未指定 `--version` 的首次安装在该模式下使用 `latest`。

命令启动时保存原始终端属性。收到 Ctrl+C 后，在退出前恢复包括密码输入期间关闭的
`ECHO` 在内的属性，并显示光标，避免调用结束后 shell 留在不可见输入状态。

交互确认必须要求用户输入完整的 ASCII `yes`；空输入、其他内容、EOF 或中断都视为拒绝并停止当前操作。如果拒绝发生在事务创建后，保留当前事务，由下次执行按中断恢复规则处理。

非交互模式只能使用对应的专用参数：

- 受管 unit 兼容变化使用 `--accept-service-change`；
- 后端摘要不一致修复使用 `lkit repair binary`。

显式 `--repository` 已经表示使用该仓库，不再要求二次确认。上述命令和专用参数不能
绕过状态损坏、初始化锁缺失、未知冲突进程、systemd unit 所有权冲突、同版本资产身份
不一致、下载校验失败或未解决事务。

密码复杂度按当前 Landscape 稳定接口固定为：

- UTF-8 字节长度至少 `8`；
- 至少包含一个 ASCII 小写字母 `a-z`；
- 至少包含一个 ASCII 大写字母 `A-Z`；
- 至少包含一个 ASCII 数字 `0-9`；
- 不额外要求特殊字符；
- 非 ASCII 字符可以出现，但不能替代上述三类 ASCII 字符。

`lkit` 必须在写初始化文件前执行同样校验。Landscape 未来改变该稳定规则时，必须同步升级安装协议或提供版本化验证能力。

密码文件必须：

- 是普通文件，打开最终路径时不得跟随符号链接，并在打开后通过文件描述符重新验证；
- 由 root 拥有；
- group 和 other 无任何权限；
- 大小不超过 `4 KiB`；
- 内容是单行有效 UTF-8，不包含 NUL；
- 读取后只移除一个行尾：末尾为 `\r\n` 时移除这两个字节，否则末尾为 `\n` 时只移除该字节；不 trim 其他空白；
- 非空并满足 Landscape 密码要求。

### 保留与变更

首次安装创建并保留 `data/landscape_init.toml`。初始化仍为 `pending` 且初始化锁尚未出现
时，该路径必须是当前受管用户所有、权限严格为 `0600` 的普通文件；生产环境的受管用户
是 root，因此对应 `root:root 0600`。缺失文件、符号链接、所有者不符或权限过宽都会
阻断后续管理操作。lkit 不解析或比较文件内容；内容是否能完成初始化由 Landscape 在
启动时判定。

初始化完成后，`landscape_init.toml` 只是保留的一次性输入，不再属于 lkit 的内容状态：

- 正常升级、同版本检查、repair 和 reconcile 不读取、不比较也不改写该文件；
- 不要求其中的 `version` 等于当前版本；
- 用户可以修改或删除该文件，无需确认或专用接受参数；
- state 不记录该文件的 SHA-256，旧 state 中历史 `config_sha256` 字段被兼容忽略。

这不改变 `.lkb` 的备份契约。`.lkb` 中的 `landscape_init.toml` 来自运行中实例的配置
导出 API，仍保留在归档中并参与归档完整性校验；回滚时使用它在新的 data 目录中重新
初始化旧版本。它与安装目录里保留的一次性 init 文件不是同一个信任判断。

如果存在数据库或 `landscape.toml`，或者安装状态记录 `initialization.status: complete`，但 `landscape_init.lock` 缺失，则属于高危异常。Landscape 可能重新读取初始化文件并清空配置；任何普通确认或 `--accept-*` 均不能绕过，安装必须停止。

首次安装提交的 `initialization.status: pending`、无数据库、无 `landscape.toml` 且无初始化锁是预期状态，不得误判为损坏。之后观察到数据库或 `landscape.toml` 已出现但初始化锁仍缺失时，立即按高危异常处理。

网络接管首次安装在确认前仍未提交。超时、确认前重启或手工 rollback 成功后，其整个
`data/` 会被删除；因此下一次 `lkit install` 可以重新提供 `--admin-user` 和
`--password-file`。如果回滚失败，必须先人工处理 `failed` 事务和残留数据，不能通过重复
提供初始化凭据绕过已有数据保护。
