# 宿主网络适配（hostnet）

## 职责

`lkit-hostnet` 是独立于 lkit-cli 的纯库 crate，负责"把选中的网络接口从宿主网络管理器中
摘除，并在回滚/卸载时恢复"。实现四个适配器：ifupdown、NetworkManager（conf.d
drop-in）、firewalld（zone XML 接口行）与 systemd-networkd（`.network` 文件移出）。

当前托管网络管理的整体行为见[网络接管](takeover.md)；本文档描述 `lkit-hostnet` 本身的
设计与测试。

## 为什么是独立 crate

- 解析、改写与恢复是纯文件逻辑，零系统依赖（不依赖 lkit-cli、不调用 systemd），
  路径全部注入，可脱离 CLI 独立测试；
- lkit-cli 的接管流程只依赖本 crate 的 trait 接口（`execute_unmanage`/`restore`），
  适配器内部逻辑的演进不影响调用方；
- systemd-networkd（`.network` 文件移出）即按此路径在同一 crate 内新增模块，调用方
  接口不变。

## 设计边界

- **只操作宿主网络配置文件**：ifupdown 的 `/etc/network/interfaces`（含 `source`
  和 `source-directory` 引用的文件）、NetworkManager 的 conf.d drop-in、firewalld 的
  zone XML、systemd-networkd 的 `.network` 文件。不直接操作接口、不调用
  `systemctl`/`nmcli`/`firewall-cmd`/`networkctl`、不碰 `ip` 命令；
- **摘除与恢复对称**：接管时备份原文件逐字副本（新建的文件记录为 created，无逐字
  副本），回滚/卸载时按 manifest 逐字覆盖恢复或删除，不依赖 diff 或补丁；
- **保守解析**：只识别文档化的语法结构，遇到无法解析的内容报错而非猜测；
- **校验交给系统工具**：crate 自身不做语义判断，通过注入的 `ifup --no-act --all`
  等工具路径做 dry-run 校验，工具缺失时返回 warning 性质结果，由调用方决定策略。

## 架构

```text
lkit-hostnet
├── lib.rs          crate 根、错误类型、公共 trait
├── ifupdown/       ifupdown 适配器
│   ├── collect.rs  文件清单收集（主文件 + source）
│   ├── parse.rs    保守解析器（ifupdown(5) 语义）
│   ├── edit.rs     改写计划与应用（原子写回）
│   ├── backup.rs   逐字备份 + manifest.json
│   └── validate.rs ifup dry-run 校验
├── nm/             NetworkManager 适配器（conf.d drop-in）
│   ├── collect.rs  conf.d 清单收集（drop-in 是否已存在）
│   └── edit.rs     drop-in 内容生成与元数据
├── firewalld/      firewalld 适配器（zone XML 接口行删除）
└── networkd/       systemd-networkd 适配器（.network 文件移出）
```

适配器实现统一的 trait（ifupdown、nm、firewalld、networkd 均已实现）。调用方应优先使用
`execute_unmanage`；分步方法保留给适配器专项测试：

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
`restore` 仍按 manifest 无条件恢复（改写条目逐字还原、created 条目删除）。工具缺失
返回 `Validation::Unavailable`，视为 warning 性质的成功结果。`FileSet` 为空或计划为空时
不创建备份目录。备份与原子写等文件操作由 ifupdown 模块提供、四个适配器共用。

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

### 反查

`IfupdownAdapter::unmanaged_interfaces(sources, manifest)` 反查当前摘除现场：现场文件
中 method 为裸 `manual` 的选中态 stanza，加上 manifest 快照中非 manual-bare 的接口
（即被本次摘除改写的）。

## NetworkManager 适配器

- **文件范围**：conf.d 目录（路径注入，生产环境 `/etc/NetworkManager/conf.d/`）中的
  `lkit-unmanage.conf`；conf.d 目录本身是符号链接、或 drop-in 是符号链接时以
  `PathSafety` 阻断。
- **改写规则**：drop-in 内容为 `[device]` 段的
  `unmanaged-devices=interface-name:<if>;...`（选中名排序去重），头部带说明注释。
  选中名含 glob 元字符（`*?[]`）或不可用字符时拒绝。drop-in 不存在时新建（权限
  0644、属主继承 conf.d 目录）；已存在（含宿主同名文件）时逐字备份后改写，恢复时
  逐字还原。
- **恢复**：新建的 drop-in 直接删除（conf.d 目录与其他文件不动）；改写的逐字还原。
- **校验**：无 dry-run 工具，`validate` 返回 `Unavailable`；运行时效果由调用方
  `nmcli general reload` 后自查。
- **反查**：`NmAdapter::unmanaged_interfaces(sources)` 读取现场 drop-in 的
  `unmanaged-devices` 条目；drop-in 缺失或不可读返回空集。

## firewalld 适配器

- **文件范围**：zones 目录（路径注入，生产环境 `/etc/firewalld/zones/`）中全部
  `*.xml`（按文件名排序）；目录或 zone 文件是符号链接时以 `PathSafety` 阻断。
- **改写规则**：删除引用选中接口的 `<interface name="..."/>` 整行——仅自闭合、单属性
  形态（两种引号），其余内容逐字节保留。删除整行形态后，选中接口仍以其他形态出现在
  任何 `<interface>` 元素中（多属性、跨行、或与整行形态混在同一文件）时保守拒绝
  整个摘除（`UnsupportedSyntax`），计划阶段即失败、不改任何文件。
- **恢复**：逐字还原改写过的 zone 文件。
- **校验**：无 dry-run 工具，`validate` 返回 `Unavailable`；运行时效果由调用方
  `firewall-cmd --reload` 重读 zone 生效。接口脱离显式 zone 后由 firewalld 默认 zone
  兜底。
- **反查**：`FirewalldAdapter::unmanaged_interfaces(manifest)` 取"manifest 快照中原有
  的整行接口名 − 现场文件仍存在的名字"差集；快照或现场文件缺失按空集处理。

## systemd-networkd 适配器

- **文件范围**：配置搜索路径（路径注入，生产环境按优先级从高到低为
  `/etc/systemd/network/`、`/run/systemd/network/`、`/usr/lib/systemd/network/`）中全部
  `*.network`（各目录内按文件名排序，目录间保持优先级序）。networkd 同名文件只有最高
  优先级者生效。`.netdev` 定义虚拟设备，选中接口均为物理接口，不收集；目录缺失时为
  no-op；目录或文件是符号链接时以 `PathSafety` 阻断。
- **改写规则**：`[Match]` 段 `Name=` 的精确名集合 ⊆ 选中集合时把文件整体移出——
  networkd 的 `[Match]` 同键条目 OR、异键 AND，`Name=` 精确集即匹配集上界，整文件移出
  不会波及未选接口。`Name=` 以 glob 引用选中接口（无法归因完整匹配集）、或同一文件
  同时引用选中与未选精确名时保守拒绝（`UnsupportedSyntax`），计划阶段即失败、不改
  任何文件；无 `Name=`（按 MAC/Driver 等匹配）的文件不按名字归因，原样跳过。移出高
  优先级文件会让低优先级目录的同名遮蔽文件生效：遮蔽文件同引选中接口时级联移出，
  引用未选接口或无法归因（glob、无 `Name=`）时保守拒绝——不猜生效后的匹配集。
- **恢复**：移出的文件按备份逐字副本重建（恢复 mode/uid/gid）；guarded 恢复中删除后
  被外部重建的文件保留并上报 `ConcurrentModification`。
- **校验**：无 dry-run 工具，`validate` 返回 `Unavailable`；运行时效果由调用方
  `networkctl reload` 重读生效。
- **反查**：`NetworkdAdapter::unmanaged_interfaces(manifest)` 取"manifest 中 original
  路径已消失的移出文件"的 `Name=` 精确名并集；文件被人工重建则不再计入。

## 备份与恢复格式

`backup(plan, dest)` 要求 `dest` 是不存在的绝对路径。改写与移出条目（`FileEdit.removed`，
整文件删除）把源文件逐字复制到 `dest/<序号>/<源文件名>`；新建条目只记录路径与元数据
（无逐字副本）。manifest 写 `dest/manifest.json`（`backup` 字段为绝对路径），记录源文件
的 mode、uid、gid；备份文件和 manifest 使用 `0600`：

```json
{
  "schema_version": 1,
  "files": [
    {
      "original": "/etc/network/interfaces",
      "backup": "/var/lib/.../backups/0/interfaces",
      "metadata": { "mode": 420, "uid": 0, "gid": 0 }
    }
  ],
  "created": [
    {
      "path": "/etc/NetworkManager/conf.d/lkit-unmanage.conf",
      "metadata": { "mode": 420, "uid": 0, "gid": 0 }
    }
  ]
}
```

`restore(manifest)` 先完整读取并验证所有备份，再按 `original` 路径逐字覆盖恢复每个
改写文件、逐字重建每个移出文件、删除每个 created 文件（目标为符号链接时 `PathSafety`
阻断），覆盖前不要求文件仍处于改写状态（幂等），用于显式回滚/卸载。事务失败使用
`restore_if_unchanged`，只恢复仍处于本次编辑结果的文件；外部漂移不会被覆盖。
恢复后接口是否立即重新配置（如 `systemctl restart networking.service`、
`nmcli general reload`）由调用方决定，本 crate 不执行。

### 原子写回

改写与恢复均采用带进程 ID 和原子序号的独占临时文件（`create_new`），写入后精确
设置权限/所有者，执行 `sync_all`、rename 和父目录 fsync；失败不跟随临时文件符号链接。

### 校验（ifupdown）

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
  - NM：drop-in 新建/删除、既有 drop-in 逐字备份与还原、guarded 恢复保留外部修改、
    确定性元数据、反查；
  - firewalld：整行删除与逐字恢复、无关 zone 不动、混合形态拒绝（不改任何文件）、
    反查差集；
  - networkd：可归因文件移出与逐字重建、混合精确名拒绝、glob 引用选中接口拒绝、
    无 `Name=` 文件跳过、反查排除被重建文件、guarded 恢复保留外部重建；
  - 备份/恢复：逐字一致、私有备份、幂等恢复、created 条目删除、guarded rollback
    保留外部修改、manifest 元数据往返、schema/符号链接拒绝；
  - 原子写回：独占临时文件、旧临时符号链接不跟随、失败路径不留残留。
- **集成测试**（crate `tests/`）：收集 → 备份 → 改写 → 校验 → 恢复 全流程；
  fixture 提供假 `ifup` 脚本验证校验分支（成功/失败/缺失三种）。
- **风险测试隔离**：涉及真实 Debian ifupdown 工具的测试标记 `#[ignore]`，只在 CI 的
  rust:bookworm 容器内以 `--ignored` 运行（`test-hostnet-ifupdown.yml`）；其余全部
  在临时目录与注入路径/假工具上进行，不触碰宿主网络环境。

## 与 lkit-cli 的集成

`crates/lkit-hostnet` 是 workspace member（不进 default-members），lkit-cli 通过
`lkit-hostnet` 依赖接入接管流程：

- runtime 注入各适配器入口与工具路径：`interfaces_file`/`ifup_command`（ifupdown）、
  `nm_conf_d`（NM drop-in 落点）、`firewalld_zones`（zone 目录）、`networkd_dir`
  （`.network` 目录）、`nmcli`/`firewall_cmd`/`networkctl`（运行时 reload 工具）。
  `networking.service`、NetworkManager、firewalld 与 systemd-networkd 都不在 lkit-cli
  的整体 stop/disable/mask 服务清单内（清单只剩 systemd-resolved），相关宿主的摘除
  完全由本 crate 的文件改写承担。
- 接管时按目录存在性选择适配器（ifupdown 主配置文件、NM conf.d、firewalld zones、
  networkd 配置目录，可组合），各自经 `execute_unmanage` 备份并改写，备份固定落 lkit
  地盘 `backups/hostnet/<ifupdown|nm|firewalld|networkd>/`；摘除先于停止宿主服务执行，
  失败即中止整个安装。文件改写后 lkit-cli 对运行中的 NM/firewalld/networkd 执行
  reload（工具缺失或失败即中止）。
- 回滚与卸载时按各 manifest 逐字恢复（NM drop-in 删除、networkd 移出文件重建），再按
  服务实况重放运行时：`networking.service` active 时 restart、运行中的
  NM/firewalld/networkd reload（尽力而为）。
- 接管生效的判定（reinit 前置校验、卸载警告）除服务状态探测外，还把地盘未恢复的
  hostnet 备份（任一适配器）视为接管特征。
- reinit 必须维持与既有接管相同的接口集合：现场集合取各适配器反查（ifupdown manual
  stanza、NM drop-in 条目、firewalld 快照差集、networkd 移出文件 Name= 精确集）的
  并集比对，换选接口需先 uninstall 再重新接管。

整体行为见[网络接管](takeover.md)。

## 后续迭代

- `unmanaged_interfaces` 从各适配器固有方法提升到 trait，以及 ifupdown 主机上 reinit
  换选接口的原地重放与谱系回滚；
- 更复杂的 shell 风格 `source` 展开，以及 bridge/bond 的其他依赖声明形式。
