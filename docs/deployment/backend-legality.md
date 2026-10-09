# 后端合法性与主线对照

本文定义受管后端二进制的**完整性**与**合法性**模型。唯一合法的后端是主线（官方
Release 及其镜像）发布的后端；任何其他来源的二进制——包括用户自编译构建、离线介质
上的文件或被外部替换的产物——无论经过什么途径部署，都不构成合法状态。

## 双身份模型

`install-state.json` 的 `assets.webserver` 记录两个身份：

| 字段 | 语义 |
| --- | --- |
| `sha256` / `size` | **完整性锚点**：最近一次 lkit 部署事务提交时落盘二进制的实际身份 |
| `official_sha256`（可空） | **主线对照身份**：主线对该版本后端在落盘形态下声明的 SHA-256；无法对照时为 `null` |

每次命令执行时现场计算磁盘二进制哈希，按以下优先级判定后端状态：

| 状态 | 判定 | 含义 |
| --- | --- | --- |
| `drifted`（漂移） | 磁盘哈希 ≠ `sha256` | 完整性破坏：二进制在 lkit 不知情的情况下被改动（手工替换、损坏或篡改） |
| `custom`（非主线） | 磁盘哈希 == `sha256`，但 `official_sha256` 缺失或 ≠ `sha256` | 完整性成立，但内容不是主线后端（自定义构建），或主线对照身份未知 |
| `official`（合法） | 磁盘哈希 == `sha256` == `official_sha256` | 落盘内容与主线逐字节一致 |

`drifted` 优先于 `custom`，`custom` 优先于 `official`。备份列表、控制台与各命令
提示使用同一组词。

合法性与完整性是两个独立的不变量：

- **完整性**回答"磁盘内容是否仍是 lkit 上次部署的那份"——锚点是本机 state；
- **合法性**回答"那份内容是否来自主线"——锚点是主线声明。

lkit 的每个部署事务（install、update、switch、repair、restore、custom）提交时都把
`sha256` 重新锚定到本次实际落盘的二进制；因此经过 lkit 的部署永远满足完整性，而
合法性取决于部署内容是否为主线产物。

## 主线与对照方式

主线是官方 GitHub Release 及其镜像仓库：镜像发布流程先按官方 `SHASUM256sum.txt`
校验原始后端、再压缩上传，保证镜像资产与官方逐字节一致（见
[发布仓库协议](../repository.md)）。对照来源按 仓库解析优先级（显式 CLI >
`config.toml` > 官方 GitHub）选取，镜像与官方在对照语义上等价。

获取落盘形态的主线哈希有三种途径：

| 数据源 | 对照方式 | 成本 |
| --- | --- | --- |
| GitHub 官方 | 读 Release 的 `SHASUM256sum.txt`（记录的即原始未压缩资产的哈希） | 只读清单，无需下载产物 |
| HTTP 镜像，manifest 含 `sha256_decompressed` | 读 manifest 字段 | 只读 manifest |
| HTTP 镜像，manifest 缺该字段 | 下载 `.zst` 资产 → 解压 → 计算哈希 | 一次完整下载与解压 |

HTTP 镜像 manifest 的 `sha256`/`size` 描述**压缩产物**，不能直接与落盘二进制比对；
`sha256_decompressed` 是协议为对照目的提供的解压后（落盘形态）哈希，规则见
[发布仓库协议](../repository.md#资产结构)。

对照可能失败（网络不可达、来源解析失败或版本不在主线）。对照失败不阻断
`lkit custom` 的部署本身，结果按"对照身份未知"（`official_sha256: null`，状态
`custom`）提交，事后可由 `lkit repair binary` 补做对照并转正。

## 命令行为矩阵

| 命令 | `drifted` | `custom` | 说明 |
| --- | --- | --- | --- |
| `install` / `update` / `switch`（升级路径） | 默认拒绝，知情确认后继续 | 默认拒绝，知情确认后继续 | 见下节"升级门槛" |
| `install --version <当前版本>`（同版本安装校验） | 拒绝，无越过参数 | 拒绝，无越过参数 | 主线管理动作，保持严格 |
| `backup create` | 允许 | 允许 | 如实快照当前内容，身份写入 metadata，列表标注 |
| `restore` | 允许（保护备份如实快照） | 允许 | 恢复后状态继承备份的合法性与完整性 |
| `uninstall` | 允许（保护备份如实快照） | 允许 | 卸载不被后端状态阻断；unit 所有权检查不变 |
| `reinit` | 允许（保护备份如实快照） | 允许 | reinit 不触碰后端，版本与资产逐字节不变 |
| `repair binary` | 允许 | 允许 | 转正入口，见下节 |
| `custom` | 允许，显式确认后继续 | 允许 | 受控部署通道，见 [`lkit custom`](../commands/custom.md) |

保护备份（switch、repair、restore、uninstall、reinit 前的 `.lkb`）始终如实快照当前
实际内容：漂移或非主线的二进制照常打包，身份与标注写入 metadata 的 `backend`
对象（见 [`.lkb` 备份与回滚](../backup/lkb-and-rollback.md#backupmetadata-schema-v1)）。

## 升级门槛与知情确认

升级路径（`install` 超过当前版本、`update`、`switch`）默认只接受 `official` 状态：

- 非 `official` 状态下发起升级时默认拒绝，错误说明当前状态（`custom` 或 `drifted`）
  并给出出路；
- 交互模式下追加一次后端替换确认，说明"当前后端不是主线构建，升级将用主线版本替换
  它，保护备份会如实记录当前内容"，输入完整 `yes` 后继续；
- 非交互模式（含 daemon 委托与控制台分发）必须显式携带
  `--accept-custom-backend` 才能继续；该参数等价于上述确认；
- 拒绝或缺少确认时返回普通失败 `1`，不创建事务、不下载资产，现场保持不变。

确认只解除这一次门槛：升级部署的是主线后端，提交后 `sha256` 与
`official_sha256` 都来自主线产物，状态自动回到 `official`。门槛期间创建的保护
备份如实记录升级前的内容，升级失败自动回滚或用户事后均可从该备份恢复。

三条不变量：

1. **默认严格**：没有显式确认，绝不从非合法状态升级；
2. **知情才越**：越过动作永远绑定一次显式确认（键盘 `yes` 或脚本明写参数）；
3. **越后即正**：越过确认后的提交结果一定是合法状态，门槛只挡一次。

## 转正与逃生路径

非 `official` 状态回到主线的标准入口是 [`lkit repair binary`](../commands/repair.md)：

1. 按仓库解析优先级获取活动版本主线后端的落盘形态哈希；
2. 与本地磁盘二进制一致 → **纯元数据转正**：只更新 state（锚点与对照身份一致化），
   不下载替换、不停止服务、不创建 `.lkb`；
3. 不一致 → 下载主线后端、校验解压替换（完整事务：保护 `.lkb`、健康检查、失败
   回滚），提交后状态合法。

覆盖的场景：

- 自定义构建恰好与主线逐字节一致（可复现构建）→ 转正后即 `official`；
- `lkit custom` 离线安装、对照身份为 `null` → 联网后 repair 补做对照，一致则转正；
- 本地二进制被替换或损坏（`drifted`）→ repair 下载主线产物替换恢复合法。

边界：活动版本不在主线存在（例如自定义的开发版本号）时，repair 无法获得对照物，
该状态**无法被转正**。回到主线的出路只有：

- 带 `--accept-custom-backend` 的升级（升级后即为合法状态）；
- 恢复一个 `official` 状态的 `.lkb` 备份；
- `lkit uninstall` 后重新 `lkit install`。

## 与备份的联动

`.lkb` metadata 携带 `backend` 对象（可空），记录备份时刻的后端身份：

```json
"backend": {
  "binary_sha256": "归档内 landscape-webserver 条目的 SHA-256",
  "official_sha256": "备份时 state 的主线对照身份（可为 null）",
  "drifted": false
}
```

- `backup list` 与控制台按该对象展示后端状态
  `official / custom / drifted / legacy`；`legacy` 表示旧版本备份不含该对象；
- restore 提交的 state：`sha256` 从解包二进制现场计算并与
  `backend.binary_sha256` 交叉校验；`official_sha256` 继承备份 metadata，恢复后
  的合法性与备份时刻一致；
- 旧备份（无 `backend` 对象）恢复后 `official_sha256` 按落盘二进制身份记录——
  旧备份只能在完整性受验证的旧机制下创建，其内容视为可信部署产物。

## 兼容规则

- **旧 state**：`official_sha256` 字段缺失时按"等于 `sha256`"处理。历史部署均来自
  主线，该规则保证既有安装升级 lkit 后不会集体降级为 `custom`；后续写入总是显式
  记录该字段。
- **旧 `.lkb`**：metadata 缺少 `backend` 对象时按 `legacy` 展示，restore 行为见
  上节。
