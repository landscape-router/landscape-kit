# `lkit migrate`

把手工部署（非 lkit 安装格式）的 Landscape Router 迁移为 lkit 受管安装。

```text
lkit migrate --from <CONFIG_DIR> [--install-dir <PATH>]
             [--repository [<BASE_URL>]]
             [--yes]
```

- `--from` 指定旧手工部署的配置目录（如 `/root/.landscape-router`）。目录必须包含
  Landscape 特征文件（`landscape.toml` 或 `landscape_init.lock`），且不是受管安装的
  `data/` 目录。
- 旧实例**必须正在运行**：迁移备份走与 `lkit backup` 相同的路径——从运行实例的
  `/api/v1/system/config/export` 导出当前配置，同时从导出响应读取后端版本。
- **备份不升级**：迁移 `.lkb` 记录旧部署导出的版本；恢复后活动版本保持该版本，
  升级由后续 `lkit switch` 完成。
- 目标安装根目录必须是全新目录：不存在 `install-state.json`，也没有遗留的
  `data/`、`releases/`、`service/` 或 `current`。
- `--repository` 只在本地缺少 `static.zip` 时用于从发布仓库下载该版本的压缩包；
  下载不可用时回退为把解压后的 `static/` 现场打包。
- 非交互模式必须显式 `--yes` 确认迁移计划。

## 迁移流程（同一事务，失败自动回滚）

1. **校验源目录**：特征文件、真实目录、非受管安装。
2. **识别运行实例**：按固定端口（TCP/UDP 53、TCP 6300、TCP 6443）定位占用进程，
   通过 `--config-dir` 参数确认它服务 `--from` 目录；端口上有无法确认身份的进程时阻断；
   实例未运行时报错并提示先启动旧实例。
3. **创建迁移备份**（`preparing`）：
   - 通过导出 API 读取当前配置与后端版本；
   - 后端二进制从 `/proc/<pid>/exe` 读取（运行中文件已被删除时仍可靠）；
   - static 目录取进程 `--web` 参数，缺省为 `<CONFIG_DIR>/static`；
   - `static.zip` 本地存在则直接使用，否则从发布仓库下载，仓库不可用时从
     `static/` 现场打包并自校验；
   - 按 `.lkb` minimal scope 生成迁移备份到 `<install-root>/backups/<id>.lkb`。
4. **确认**：展示源目录、后端版本（不升级）、目标管理方式和安装根目录；拒绝时
   不创建事务、不写任何文件。
5. **停止旧实例**（`stopping`）：
   - systemd 可用时扫描 unit 文件，按 `ExecStart` 的 `--config-dir` 匹配发现旧 unit：
     - 唯一匹配：`stop` + `disable`；unit 原件位于 `/etc/systemd/system` 时把文件
       移入事务目录（`mask` 会与受管 `landscape-router.service` 注册冲突），
       其他位置的 unit 走 `stop + disable + mask`；
     - 多匹配：阻断，要求先手工清理；
     - 无匹配：实例为前台进程，要求用户确认已停止（`lkit` 不验证运行态）。
   - 旧实例仍存活时同样要求确认（前台进程）。
6. **重建安装**（`activating`）：从迁移备份解包重建 `releases/<version>`（二进制、
   `static.zip`、`static/`），创建空 `data/`，写入导出的 `landscape_init.toml`
   （`0600`），恢复 `geo_tmp`，原子创建 `current`。
7. **接管运行态**（`verifying`，systemd 模式）：写入受管 unit 原件、注册、启用、
   启动，完成 180 秒启动检查与 10 秒稳定观察。
   pending/未验证状态并输出参考启动命令。
8. **提交**：状态记录被迁移版本、备份内资产身份、`initialization.status: complete`
   （systemd 模式，初始化锁由新实例首次启动生成）与目标管理方式。

成功后输出：接管版本、迁移备份 ID，以及旧部署目录"未被修改、服务已停止，可自行
清理"的提示。

## 失败与恢复

- 停止旧实例前失败：事务标记 `failed`，旧实例未受任何影响，退出码 `1`。
- 停止后失败（含健康检查失败）：自动回滚——停止/注销新受管 unit、恢复
  `/etc/resolv.conf`、把旧 unit 文件放回原位（或 `unmask`）并按事务前
  enabled/active 状态重启，再清理新根内容；回滚成功返回 `5`，回滚失败返回 `6`。
- 前台实例场景无法自动重启旧实例，由用户负责。
- 中断恢复按事务阶段处理：`preparing` 标记 `failed`（迁移备份保留）；
  `prepared`/`stopping` 恢复旧 unit 与事务前 systemd 状态；
  `activating`/`verifying` 执行与失败相同的回滚。

## 旧部署去留

迁移**不删除、不修改**旧部署目录和旧二进制；旧 unit 在成功后保持停止状态。
用户确认不再需要后自行清理。`lkit uninstall` 只管理 lkit 安装根目录，不涉及旧部署。
