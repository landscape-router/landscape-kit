# 宿主网络适配（hostnet）

## 职责

`lkit-hostnet` 是独立于 lkit-cli 的纯库 crate，负责"把选中的网络接口从宿主网络管理器中
摘除，并在回滚/卸载时恢复"。当前只实现 ifupdown 适配器；NetworkManager 和
systemd-networkd 仍是后续阶段。

当前托管网络管理的整体行为见[网络接管](takeover.md)；本文档描述 `lkit-hostnet` 本身的
设计与测试。

## 为什么是独立 crate

- 解析、改写与恢复是纯文件逻辑，零系统依赖（不依赖 lkit-cli、不调用 systemd），
  路径全部注入，可脱离 CLI 独立测试；
- 阶段一先以独立 crate 交付全部逻辑与测试，测试通过后再接入 lkit-cli（阶段二），
  降低对现有接管流程的改动风险；
- 后续 NetworkManager（conf.d `unmanaged-devices`）、systemd-networkd（`.network`
  文件移出）等适配器在同一 crate 内新增模块即可，调用方接口不变。

## 设计边界

- **只操作宿主网络配置文件**：ifupdown 的 `/etc/network/interfaces`（含 `source`
  和 `source-directory` 引用的文件）。不直接操作接口、不调用
  `systemctl`、不碰 `ip` 命令；
- **摘除与恢复对称**：接管时备份原文件逐字副本，回滚/卸载时按 manifest 逐字覆盖
  恢复，不依赖 diff 或补丁；
- **保守解析**：只识别文档化的语法结构，遇到无法解析的内容报错而非猜测；
- **校验交给系统工具**：crate 自身不做语义判断，通过注入的 `ifup --no-act --all`
  等工具路径做 dry-run 校验，工具缺失时返回 warning 性质结果，由调用方决定策略。

## 架构

```text
lkit-hostnet
├── lib.rs          crate 根、错误类型、公共 trait
├── ifupdown/       ifupdown 适配器（阶段一实现）
│   ├── collect.rs  文件清单收集（主文件 + source）
│   ├── parse.rs    保守解析器（ifupdown(5) 语义）
│   ├── edit.rs     改写计划与应用（原子写回）
│   ├── backup.rs   逐字备份 + manifest.json
│   └── validate.rs ifup dry-run 校验
├── nm/             NetworkManager 适配器（后续迭代）
└── networkd/       systemd-networkd 适配器（后续迭代）
```

适配器实现统一的 trait（阶段一仅实现 ifupdown 分支）。调用方应优先使用
`execute_unmanage`；分步方法保留给适配器专项测试和后续适配器实现：

```rust
pub trait HostNetworkAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError>;
    fn plan_unmanage(
        &self,
        file_set: &FileSet,
        selected: &[String],
    ) -> Result<EditPlan, HostNetError>;
    fn apply(&self, plan: &EditPlan) -> Result<EditOutcome, HostNetError>;
    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError>;
    fn restore(&self, manifest: &Manifest) -> Result<(), HostNetError>;
    fn restore_if_unchanged(
        &self,
        manifest: &Manifest,
        plan: &EditPlan,
    ) -> Result<(), HostNetError>;
    fn validate(&self, file_set: &FileSet, tools: &ToolPaths) -> Result<Validation, HostNetError>;
    fn execute_unmanage(
        &self,
        sources: &FileSources,
        selected: &[String],
        backup_dir: &Path,
        tools: &ToolPaths,
    ) -> Result<UnmanageOutcome, HostNetError>;
}
```

`execute_unmanage` 固定执行 收集 → 改写计划 → `backup` → `apply` → `validate`。
backup 成功后的 apply 错误、validate 错误或 dry-run 非零退出都会自动执行
`restore_if_unchanged`：仍处于本次编辑结果的文件才会恢复，仍是原始快照的文件跳过，
检测到其他外部内容或元数据时保留外部修改并返回 `RecoveryFailed`。显式调用
`restore` 仍按 manifest 无条件恢复。工具缺失返回 `Validation::Unavailable`，视为
warning 性质的成功结果。`FileSet` 为空或计划为空时不创建备份目录。

`FileSources`、backup 目录和 manifest 中的路径必须是绝对路径。配置入口和 source 最终
文件必须是普通非符号链接文件；符号链接会在任何写入前以 `PathSafety` 阻断。

## ifupdown 适配器

### 文件范围

- 主文件 `/etc/network/interfaces`（路径注入，默认即该路径）；
- 主文件中 `source <glob>` 和 `source-directory <glob>` 指令展开的匹配文件（Debian
  默认 `source /etc/network/interfaces.d/*`）；source-directory 只收集文件名符合
  `[A-Za-z0-9_-]+` 的普通文件；
- 文件中不包含选中接口 stanza 时视为"该接口不由 ifupdown 管理"：不修改任何文件。

### 解析规则（ifupdown 0.8 常用语法）

- 行首 `#` 为注释，空行与空白行原样保留；顶层关键字允许前导空白；支持 LF/CRLF 行尾，解析时去掉行尾
  `\r` 但改写仍保留未修改物理行的原始字节；
- `auto <iface...>`、`allow-* <iface...>` 声明接口的自动选择组；接管时从这些行中
  删除已选接口，空行整体删除；
- `iface <iface> <family> <method>` 开启一个接口块，可带 `inherits <template>`；
  其后跟随的选项可以缩进，也可以不缩进；下一个标准顶层关键字
  （`iface`、`auto`、`allow-*`、`mapping`、`rename`、`source`、`source-directory`、
  `no-auto-down`、`no-scripts`）结束当前块；空行和注释不结束当前块；
- 末尾为反斜杠的行会与下一物理行合并；无法结束的续行、未知顶层语句和畸形 stanza
  返回解析错误，不修改任何文件；
- 同一接口可同时存在 `inet` 与 `inet6` 块，分别改写；
- 同一接口在多个文件中（主文件与 interfaces.d）存在重复块：全部改写并全部备份；
- 无法归类的行、畸形块结构、`source` 展开失败：返回解析错误，不修改任何文件。

### 改写规则

对每个选中接口（WAN + 全部选中 LAN）的每个 `iface` 块：

1. `method` 改写为 `manual`（如 `iface eth0 inet static` → `iface eth0 inet manual`）；
2. 删除 `inherits` 和该块的所有选项物理行；
3. 从 `auto`、所有 `allow-*`、`no-auto-down`、`no-scripts` 行删除选中接口，避免
   networking.service 或 ifupdown hook 再次处理这些接口；剩余接口和顺序保留。

已处于 `manual`、无 inherits 且无选项的块跳过改写，多次接管幂等；选中接口使用
`ppp` 方法时拒绝改写。mapping、rename 模式、其他 stanza 的 `inherits`、
`bridge_ports`/`bond-slaves` 依赖选中接口时拒绝改写。改写与恢复均为独占临时文件 +
rename 原子写回，恢复 mode/uid/gid；ACL/xattr 不在当前范围。

### 备份与恢复格式

`backup(plan, dest)` 要求 `dest` 是不存在的绝对路径，把每个待改写文件逐字复制到
`dest/<序号>/<源文件名>`，并写 `manifest.json`（`backup` 字段为绝对路径）。清单还
记录源文件的 mode、uid、gid；备份文件和 manifest 使用 `0600`：

```json
{
  "schema_version": 1,
  "files": [
    {
      "original": "/etc/network/interfaces",
      "backup": "/var/lib/.../backups/0/interfaces",
      "metadata": { "mode": 420, "uid": 0, "gid": 0 }
    }
  ]
}
```

`restore(manifest)` 先完整读取并验证所有备份，再按 `original` 路径逐字覆盖恢复每个
文件，覆盖前不要求文件仍处于改写状态（幂等），用于显式回滚/卸载。事务失败使用
`restore_if_unchanged`，只恢复仍处于本次编辑结果的文件；外部漂移不会被覆盖。
恢复后接口是否立即重新配置（如 `systemctl restart networking.service` 重新执行
`ifup -a`）由调用方决定，本 crate 不执行。

### 原子写回

改写与恢复均采用带进程 ID 和原子序号的独占临时文件（`create_new`），写入后精确
设置权限/所有者，执行 `sync_all`、rename 和父目录 fsync；失败不跟随临时文件符号链接。

### 校验

- `validate` 调用注入的 `ifup --no-act --interfaces=<主文件> --all` 对编辑后的文件集合
  做 dry-run；
- 工具路径缺失或不可执行：返回 `Validation::Unavailable`（warning 性质），
  不阻断调用方；
- 分步调用时，dry-run 非零退出返回 `Validation::Failed(stderr)`；事务入口会自动恢复
  备份并返回 `ValidationFailed`；
- 真实 Debian 容器测试使用 `ifup --no-act --interfaces=<path> --all` 验证改写后的文件；
  fake ifup 测试同时断言参数契约。

## 错误模型

独立 `HostNetError`（thiserror），至少包含：

- `UnreadableFile(path, io)`——读取/写入失败；
- `UnsupportedSyntax(path, line, reason)`——解析失败；
- `UnsupportedMethod(path, iface, method)`——`ppp` 等不支持的方法；
- `SourceExpansionFailed(path, source)`——`source` 展开失败；
- `AtomicWriteFailed(path, io)`——tmp/rename 失败；
- `ValidationFailed(exit, stderr)`——dry-run 校验失败并已成功恢复；
- `ConcurrentModification(path)`——plan/backup/apply 之间文件内容或元数据发生变化；
- `RecoveryFailed(operation, recovery)`——操作失败且恢复也失败；
- `PathSafety(path)`——路径不是绝对路径、目标已存在或文件类型不安全等。

## 测试策略

- **单元测试**（crate 内，`std::env::temp_dir()` 建临时目录，不新增 dev-dependencies）：
  - 解析矩阵：注释、空行、缩进/非缩进选项、续行、`auto`/`allow-*`、`mapping`、
    `rename`、`inherits`、interfaces.d `source`、`source-directory`、重复 stanza、
    `inet`+`inet6` 双块、损坏文件、畸形块和不安全 shell 展开拒绝；
  - 改写：`static`/`dhcp`/`manual` 等 method 到 `manual`、删除自动选择项和选项物理
    行、注释与无关接口逐字节保留、已 manual 的幂等、`ppp`/mapping/rename/bridge/bond
    拒绝、内容漂移拒绝、mode/uid/gid 保留；
  - 备份/恢复：逐字一致、私有备份、幂等恢复、guarded rollback 保留外部修改、manifest
    元数据往返、schema/符号链接拒绝；
  - 原子写回：独占临时文件、旧临时符号链接不跟随、失败路径不留残留。
- **集成测试**（crate `tests/`）：收集 → 备份 → 改写 → 校验 → 恢复 全流程；
  fixture 提供假 `ifup` 脚本验证校验分支（成功/失败/缺失三种）。

## 阶段划分

- **阶段一（当前）**：`crates/lkit-hostnet` 独立交付，含 ifupdown 适配器逻辑、
  事务入口、测试和独立 Debian ifupdown smoke；注册为 workspace member（不进
  default-members）；不修改 lkit-cli。
- **阶段二（测试通过后另行实施）**：lkit-cli 增加 `lkit-hostnet` 依赖并接入接管
  流程——`networking.service` 从 `HOST_SERVICES` 移除、runtime 注入文件路径与
  工具路径、接管时备份并改写、回滚与卸载时恢复并重启 `networking.service`、
  更新 e2e 与场景文档。接入细则见[网络接管](takeover.md)。

## 后续迭代

- NetworkManager 适配器：conf.d `[device] unmanaged-devices=` 覆盖文件；
- systemd-networkd 适配器：移出匹配选中接口的 `.network` 文件；
- 更复杂的 shell 风格 `source` 展开，以及 bridge/bond 的其他依赖声明形式。
