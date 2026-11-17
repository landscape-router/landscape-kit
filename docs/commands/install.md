# `lkit install`

仅用于首次安装 Landscape Router。lkit 地盘(`/root/.lkit/`)中已经存在有效安装状态时
返回参数使用错误(单实例约束,见[安装布局与状态](../deployment/layout-and-state.md)),
并提示使用 `update`、`switch`、`repair` 或 `reconcile`,或先 `lkit uninstall`。

```text
lkit [--non-interactive] install [--version <VERSION>]
             [--repository [<BASE_URL>]]
             [--install-dir <PATH>] [--admin-user <NAME>]
             [--password-file <PATH>]
             [--force] [--takeover-network]
```

- 未指定版本时安装最新 stable 版本；`--version latest` 与其等价。
- `--non-interactive` 是全局参数，可放在子命令前或后；它显式禁止打开终端、输入提示和
  动态终端进度。缺少必须的密码文件或确认参数时直接失败。
- `--repository` 使用默认 HTTP 镜像；`--repository github` 使用官方 GitHub 仓库；带值时
  使用指定 protocol v1 HTTP 仓库；缺省时按 显式 CLI > `config.toml` > 官方 GitHub 的
  优先级解析来源（预置配置生效，缺失时官方 GitHub）。
- 仓库来源不写入 `state/install-state.json`，`lkit install` 也不创建或更新 `config.toml`；
  该文件位于 lkit 地盘(`/root/.lkit/config.toml`)，完全由用户维护，只影响后续命令未
  显式指定 `--repository` 时的缺省来源，见[配置文件](../deployment/config.md)。
- 首次安装完成时提示 flare L2 恢复通道就绪：daemon（`lkit self install` 部署）恒常托管
  的 flare 服务端在 Landscape 网络配置错误（含接管失败）时仍可用，psK 用 `lkit flare
  setup` 查看或设置，见[flare 协议](../flare/protocol.md)。
- `--force` 不删除文件，只显示规范化安装根目录并要求用户自行清理。
- `--takeover-network` 仅用于首次安装，要求受支持的服务后端、启用且运行中的 lkit daemon 和交互终端。它让用户选择 WAN/LAN
  接口，并在 Landscape 健康后进入待确认状态；完整行为见[网络接管](../network/takeover.md)。
  管理控制台的 Install 面板始终启用该模式（开关暂隐藏，见代码中的
  `TODO(network-takeover)`）；CLI 的 `--takeover-network` 仍为显式参数。
- `--takeover-network` 只允许目标 `data/` 目录不存在或为空；已有数据必须先由对应的
  `lkit network rollback` 清理。确认前重启会触发 boot rollback，不会让本次安装继续等待。
- 已安装环境的版本更新、切换、修复和状态变更不再通过本命令的互斥 flags 表达。
- `--install-dir` 指定 landscape 应用安装根目录（默认 `/root/.lkit/landscape`）；lkit
  元数据（状态、事务、备份、锁）固定写入 `/root/.lkit/`，见
  [安装布局与状态](../deployment/layout-and-state.md)。安装提交后 landscape 根位置
  记录在 `install-state.json`，后续命令从状态发现根目录，不再接收 `--install-dir`。

如果安装根目录存在未完成的网络接管事务，`install` 不自行确认、回滚或删除数据，只提示
使用 `lkit network status`、`lkit network confirm` 或 `lkit network rollback`。`--admin-user`
和 `--password-file` 仍只允许真正的首次安装；回滚成功且未提交数据已清理后，才可再次提供
这些参数。

安装不按发行版名称设置白名单；使用 glibc 的 Linux 主机在通过完整运行能力预检后可以
继续。依赖缺失错误会按检测到的 `apt`、`dnf`/`yum`、`pacman` 或 `zypper` 显示安装建议。

安装前必须运行兼容的 lkit daemon；CLI 使用 `sudo lkit self install` 部署，自动化可用
`sudo lkit --non-interactive self install`。daemon 未运行时安装返回明确的部署提示。
人工安装建议先单独安装 `lkit`，再从终端运行 `sudo lkit`，按提示部署 daemon 后从侧栏进入 Install
面板。`lkit install` 保留为命令模式；没有 `--password-file` 时仍通过 `/dev/tty` 隐藏读取
密码。无 TTY 的自动化环境必须显式使用 `--non-interactive`，并提供 root 所有且权限为
`0400` 或 `0600` 的 `--password-file`。

交互终端中，部署前 warning 在密码提示前分行显示，隐藏输入的两次提示各自占一行。
下载后端和静态资源时分别显示包含已下载字节数、总字节数、百分比、速率和 ETA 的进度条；
stderr 不是终端时不输出动态进度，避免污染脚本日志。

命令模式和控制台使用同一原生 daemon 操作入口。CLI/TUI 在提交前收集凭据与网络
计划，安装根和来源按前端选择绑定；daemon 不读取发起会话的终端、工作目录或环境。
运行中的 daemon 必须声明安装能力，否则在写入请求前拒绝。`--force` 仅在前端显示
人工清理提示，不提交安装请求。

操作记录不包含密码。密码通过独立 `0600` 文件交接，目录为 `0700`；排队取消、
领取读取和中断整理时删除。恢复使用持久化事务，不重放安装请求或重新解析 latest。
操作 ID 在提交后输出，可用 `lkit operation status/list` 查询。Ctrl+C 或 SSH 断开
只退出观察；排队与下载阶段可通过 `operation cancel` 或 TUI 的 Esc 取消，准备与
激活阶段拒绝取消。

首次安装不要求已有安装状态。daemon 在安装锁下重新验证单实例约束、安装根、目录
安全、服务与端口冲突；存在未完成事务时先按恢复规则处理。已有 `current` 而无安装
状态的目录不能被当作干净安装。下载使用事务暂存目录，取消只清理本次暂存数据，
不删除已有 release 缓存。修改前保存被覆盖的目标目录及服务、DNS 恢复依据；失败或
崩溃时恢复这些依据，恢复失败保留非终态供 daemon 重试。

普通安装以安装状态与事务提交为成功依据；网络接管安装进入
`awaiting_confirmation`，前端返回 `0` 或显示待确认屏，此时尚未提交。确认按钮绑定
该事务，提交或回滚后更新原操作结果；daemon 重启不延长网络确认期限。
