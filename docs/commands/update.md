# `lkit update`

交互选择来源和目标 stable 版本，确认后向 daemon 提交 `switch` 操作。CLI／TUI 负责
解析目标与确认，daemon 直接执行切换、备份、服务管理、健康检查和恢复，不启动 update
worker，也不读取终端。操作类型为 `switch`、操作 schema 为 v3，事务类型为 `switch`、
事务 schema 为 v8；执行规则见 [`lkit switch`](switch.md)。

```text
lkit update [--version <VERSION>] [--repository [<BASE_URL>]]
            [--accept-service-change] [--allow-no-backup]
```

landscape 根从 `install-state.json` 发现，命令不接收 `--install-dir`。来源选择不创建、
更新或删除 `config.toml`。

## 交互流程

1. **渠道**：未显式指定 `--repository` 时，在前端选择来源。配置存在且有效时首项为
   “当前来源”，直接 Enter 接受；配置不存在时默认官方 GitHub；配置损坏时阻断默认
   来源解析，提示修复配置或显式指定来源。可选官方 GitHub、默认 HTTP 镜像、自定义
   HTTP 仓库。显式 `--repository` 跳过渠道提问。
2. **解析目标**：前端读取安装根、活动版本与完整安装状态摘要，将来源解析为具体的
   provider kind 和 location，再查询该来源的 `latest` 或指定 stable 版本。此阶段只
   读取状态和发布元数据，不提交操作、不创建事务、不下载版本资产，也不持有安装锁
   等待用户输入。
3. **比较与确认**：
   - 相同：输出已是最新并返回 `0`，不提交操作、不创建事务、不下载资产；不执行
     switch 的同版本资产校验，不持久化所选来源。
   - 更低：返回参数错误 `2`，不提交操作或创建事务。
   - 更高：展示具体来源及 `当前 <X> → 目标 <Y>`，要求完整 `yes` 确认。拒绝时返回
     `1`，不提交操作、不创建事务、不下载资产，不修改安装现场。
4. **提交**：确认绑定精确版本 `<Y>`、具体来源、安装根、原活动版本与状态摘要，携带
   `accept_service_change`、`allow_no_backup` 的显式取值，提交一个持久化 `switch`
   操作并显示操作 ID。请求不携带 `latest`、待解析的默认来源或任意命令行。
5. **执行**：daemon 领取后重新核对安装根、活动版本和状态摘要；现场变化时失败，
   用户重新读取现场并确认，不自动把请求改成针对新现场的升级。daemon 只从固定来源
   查询已确认版本的元数据并校验资产，不重新查询 `latest` 或根据默认配置改变来源。

确认后仓库发布新的 latest、默认来源或默认镜像配置发生变化，都不能改变本次请求的
目标版本与仓库地址。精确版本不可获取时报告失败，不改选其他版本或来源。版本固定不
表示跳过 daemon 的元数据查询、资产完整性、服务归属或初始化安全检查。

## 授权与非交互环境

升级确认只授权展示的来源、版本和安装现场，不隐含接受服务定义变化或无备份切换。
`--accept-service-change` 与 `--allow-no-backup` 按 switch 规则处理，daemon 不再询问
用户是否手工停止服务，也不从前端继承环境或凭据。

命令行 update 的渠道选择与升级确认要求交互终端。无 `/dev/tty` 或全局
`--non-interactive` 时返回 `1`，提示使用非交互入口：

```text
lkit switch --version latest
```

需要指定来源时追加 `--repository <BASE_URL>`。update 提交前检查 `switch-v1.pid`
能力；daemon 不可用或不支持协议时明确失败，不回退到 worker 或前台执行。

## 控制台分发

[Update 面板](../interaction/console.md)在后台解析版本，只有解析成功且目标更高时
显示确认层。确认层显示精确版本和具体来源；修改来源或版本输入、刷新解析结果会使
原确认失效。Enter 提交该确认上下文，Esc 关闭确认层且不提交操作。

共享分发使用已解析的精确版本、具体来源和确认时的安装状态摘要，不能用表单中的
`latest` 替换精确版本，也不能在提交时重新读取 state 并把新摘要当成已获确认的现场。
隐藏标记 `--console-confirmed` 仅表示终端交互已完成；它不替代完整确认上下文，也不
跳过 daemon 的校验。缺少精确目标、具体来源或确认上下文的内部请求直接拒绝。

## 进度、断开与恢复

提交后 CLI／TUI 按操作 ID 观察持久化阶段、下载进度和结果。前端退出、Ctrl+C 或 SSH
断开只停止观察；重连查询同一操作，不自动重新提交。显式取消通过
`lkit operation cancel <ID>` 或控制台的取消操作提交：排队和下载阶段允许取消，进入
preparing、备份、停服务及后续阶段后拒绝取消。清理完成后才报告 cancelled。

daemon 重启后按 switch 事务恢复；事务已提交而操作结果尚未保存时只补写结果。
回滚失败保留证据、阻止后续修改并继续恢复。退出码沿用 switch：成功 `0`、普通失败
`1`、参数错误 `2`、已回滚 `5`、恢复失败 `6`、已取消 `130`。

## 与 `lkit switch` 的关系

update 的交互与只读解析发生在前端，执行阶段复用 switch 操作队列和工作流，不定义
独立操作类型或事务类型。直接调用 `switch --version latest` 由 daemon 领取时解析
latest；update 的 latest 在前端展示和确认前解析，执行时固定为已确认的精确版本。

需要在同版本校验其他来源的资产时，使用 `lkit switch --version <CURRENT>
--repository <BASE_URL>` 或 `lkit reconcile --repository <BASE_URL>`，不要依赖
update 的“已是最新”路径。
