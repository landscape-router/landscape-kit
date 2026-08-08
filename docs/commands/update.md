# `lkit update`

将现有安装更新到目标 stable 版本，并先询问用户确认。默认目标为所选渠道的最新 stable
版本；`--version` 可以固定具体目标。update 是 [`lkit switch`](switch.md) 的交互式薄
封装：确认后复用 switch 的完整流水线（事务、`.lkb` 备份、systemd 托管、健康检查与自动
回滚），不定义独立事务类型，事务 `operation` 仍为 `switch`。

```text
lkit update [--version <VERSION>] [--repository [<BASE_URL>]]
            [--install-dir <PATH>] [--accept-service-change]
            [--allow-no-backup]
```

## 交互流程

1. **渠道**：未显式指定 `--repository` 时，用交互选择询问读取渠道。`config.toml`
   存在且有效时首个选项是其中记录的来源，直接回车即可接受；文件不存在时选项从
   官方 GitHub 开始（默认选中），不显示"当前来源"项；文件存在但损坏时报错阻断，提示
   修复或删除该文件。同时提供官方 GitHub、默认 HTTP 镜像和自定义 HTTP 仓库
   （protocol v1）。显式 `--repository` 跳过该提问，语义与 `lkit switch` 相同。
2. **解析目标**：默认解析所选渠道的 `latest`；`--version <VERSION>` 时解析指定 stable
   版本。解析发生在任何事务或备份之前。
3. **比较与确认**：目标版本与当前 `active_version` 比较：
   - 相同：输出已是最新，返回 `0`，不创建事务、不下载任何资产，也不验证或持久化所选仓库来源；
   - 更低：返回参数使用错误（退出码 `2`），不创建切换事务（沿用 switch 的降级规则）；
   - 更高：展示 `当前 <X> → 目标 <Y>` 并要求输入完整 `yes` 确认。拒绝这次升级确认时返回
     退出码 `1`，不创建事务、不下载、零副作用；无 systemd 环境在后续仍按 switch 规则
     询问用户是否已停止外部 Landscape。
4. **执行**：确认后复用 `lkit switch --version <Y> [--repository ...]` 的流水线。备份、
   回滚、systemd worker、退出码 `0/1/2/5/6` 与无 systemd 环境的停机确认语义全部与
   switch 一致，见 [`lkit switch`](switch.md)。

## 非交互环境

`lkit update` 的渠道选择与升级确认要求交互终端。无 `/dev/tty` 或显式
`--non-interactive` 时返回普通失败（退出码 `1`），并提示改用：

```text
lkit switch --version latest
```

需要指定 HTTP 仓库时，再追加 `--repository <BASE_URL>`；不带该参数时 switch 按
显式 CLI > `config.toml` > 官方 GitHub 的优先级解析来源（文件缺失时使用官方 GitHub，
见[配置文件](../deployment/config.md)）。

## 控制台分发

裸 `lkit` 控制台的 Update 面板把渠道选择与升级确认在 TUI 内完成（见
[交互控制台](../interaction/console.md)），随后以隐藏的 `--console-confirmed` 分发
结构化 `Update` 请求。该标志下：

- 渠道选择与 `yes` 确认都在 TUI 内完成，命令不再打开 `/dev/tty`——worker 是独立
  进程，无法读取 TUI 键盘输入，继续交互确认会阻塞；
- 未显式 `--repository` 时按 switch 规则解析来源（显式 CLI > `config.toml` > 官方
  GitHub），面板总是显式传递所选来源，因此正常情况下不会走到该回退；
- switch 流水线内部的交互确认（如无 systemd 环境的“已停止实例”确认）同样视为已确认；
- 目标解析、比较与执行仍在命令内完成，`--console-confirmed` 只跳过交互步骤。

## 与 `lkit switch` 的关系

- `lkit update` 是交互式升级入口，switch 是确定性的脚本入口；两者复用同一 switch 流水线。
- 需要无人值守执行，或不需要渠道选择与升级确认时使用 `lkit switch`；固定版本和自动回滚
  语义本身也可通过 update 获得。
- update 本身不新增事务类型、不改变备份策略、不改变退出码契约。

需要在同版本上改用新的 HTTP 仓库来源时，使用 `lkit switch --version <CURRENT> --repository <BASE_URL>`
或 `lkit reconcile --repository <BASE_URL>`，不要依赖 update 的“已是最新”路径。
