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
  运行实例不支持该 API（旧版本，`404`）时迁移明确失败并提示先升级旧部署。
- 委托切换要求 daemon 运行（root 下）；前台只做前置检查，最后把已 `prepared`
  的事务交给 worker 执行切换，用户能看到迁移进度。
- **备份不升级**：迁移 `.lkb` 记录旧部署导出的版本；恢复后活动版本保持该版本，
  升级由后续 `lkit switch` 完成。
- 单实例约束：lkit 地盘必须无已提交状态（不存在 `install-state.json`），landscape
  安装根必须是全新目录：没有遗留的 `data/`、`releases/`、`service/` 或 `current`。
- 迁移备份的 `static.zip` 由 `create_backup` 从旧部署的 `static/` 现场打包
  （与备份内 `static/` 树同源，含自校验）。
- 非交互模式必须显式 `--yes` 确认迁移计划。

## 迁移流程（同一事务，失败自动回滚）

迁移在 root 下分两阶段执行：**前置检查在发起进程内直接运行**（用户能看到每一步
进度），只有**切换阶段委托 daemon worker** 执行（root 下 daemon 未运行时按
`lkit self install` 提示快速失败；非 root 或测试 runtime 整条流程内联）。

1. **校验源目录**（前台）：特征文件、真实目录、非受管安装。
2. **识别运行实例**（前台）：按固定端口（TCP/UDP 53、TCP 6300、TCP 6443）定位
   占用进程。身份确认按 cmdline 的 config 目录参数（`--config-dir` 或短形式
   `-c`）是否等于 `--from` 目录；不带参数的裸部署（如 `ExecStart=/root/landscape-webserver`）
   回退到可执行文件身份（位于源目录内，或文件名含 `landscape-webserver`），
   随后的导出 API 校验（`--from/landscape_api_token`）是最终防线。端口上有无法
   确认身份的进程时阻断；实例未运行时报错并提示先启动旧实例。
3. **导出 API 支持检查**（前台）：调用 `/api/v1/system/config/export`。返回 `404`
   说明部署的 Landscape 版本不提供 config export API，报 `ExportUnsupported` 并
   提示先升级旧部署再迁移；其他失败按导出失败处理。
4. **创建迁移备份**（前台，`preparing`）：
   - 通过导出 API 读取当前配置与后端版本；
   - 后端二进制从 `/proc/<pid>/exe` 读取（运行中文件已被删除时仍可靠）；
   - static 目录取进程 `--web` 参数，缺省为 `<CONFIG_DIR>/static`；
   - 按 `.lkb` minimal scope 生成迁移备份到 `/root/.lkit/backups/<id>.lkb`，其中
     `static.zip` 从 static 目录现场打包并自校验；
5. **确认**：展示源目录、后端版本（不升级）、目标管理方式和安装根目录；拒绝时
   不创建事务、不写任何文件。前台步骤之间检查中断，`prepared` 前 Ctrl+C 中止
   并标记事务 `failed`（迁移 `.lkb` 保留）。
6. **委托切换**（`prepared` 后，daemon worker）：前台把事务标记 `prepared` 并以
   内部参数 `--resume <事务 id>` 委托；worker 认领事务后只执行切换：
   1. **停止旧实例**（`stopping`）：
      - 旧 unit 若以**普通文件**直接注册在受管路径 `/etc/systemd/system/landscape-router.service`
        上（旧安装器形态），systemd 注册的所有权保护会拒绝覆盖；实例识别已确认
        该 unit 属于旧部署（`ExecStart` 匹配源目录），先 `stop` + `disable` 并把
        文件移入事务目录，再按「未注册」接管（失败回滚放回原位）。其他形态的
        systemd 所有权冲突（无关文件、未识别的 unit）仍阻断。
      - systemd 可用时扫描 unit 文件，按 `ExecStart` 的 config 目录参数
        （`--config-dir` 或 `-c`）匹配发现旧 unit；unit 不带 config 参数时
        回退为按已识别实例的 cgroup 反查所属 unit（已安装才接管）：
        - 唯一匹配：`stop` + `disable`；unit 原件位于 `/etc/systemd/system` 时把
          文件移入事务目录（`mask` 会与受管 `landscape-router.service` 注册冲突），
          其他位置的 unit 走 `stop + disable + mask`；
        - 多匹配：阻断，要求先手工清理；
        - 无匹配：实例为前台进程，要求用户确认已停止（`lkit` 不验证运行态）。
      - 旧实例仍存活时同样要求确认（前台进程）。委托 worker 的确认由前台
        计划确认结果覆盖（`--console-confirmed`），worker 内不再弹任何交互确认。
   2. **重建安装**（`activating`）：从迁移备份解包重建 `releases/<version>`
      （二进制、`static.zip`、`static/`），创建空 `data/`，写入导出的
      `landscape_init.toml`（`0600`），恢复 `geo_tmp`，原子创建 `current`。
   3. **接管运行态**（`verifying`，systemd 模式）：写入受管 unit 原件、注册、
      启用、启动，完成 180 秒启动检查与 10 秒稳定观察。
   4. **提交**：状态记录被迁移版本、备份内资产身份、
      `initialization.status: complete`（systemd 模式，初始化锁由新实例首次启动
      生成）与目标管理方式。

成功后输出：接管版本、迁移备份 ID，以及旧部署目录"未被修改、服务已停止，可自行
清理"的提示。

## 取消（Ctrl+C）

- **前台阶段**（`prepared` 之前）：任意步骤之间检查中断，Ctrl+C 中止并标记事务
  `failed`，旧实例不受影响，迁移 `.lkb` 保留。
- **切换阶段**（委托 worker）：迁移的切换完全由事务保护，**允许取消**。Ctrl+C
  后前台写 cancel 文件，daemon 对 worker 进程组发 SIGTERM；worker 在安全点
  （阶段边界与健康检查等待期间）感知后自动回滚——停止/注销新受管 unit、恢复
  旧 unit 并按事务前 enabled/active 状态重启，前台等待回滚收尾后以 `130` 退出，
  输出"迁移已取消；已自动恢复旧实例"。回滚超过 daemon 宽限期（5 秒）时 worker
  被强杀，事务停在当前阶段，下次 `lkit migrate` 按中断恢复继续处理。
- 内联路径（非 root / 测试 runtime）同样在切换检查点响应 Ctrl+C 并回滚。

## 失败与恢复

- 停止旧实例前失败（含导出 API 不支持、前台阶段中断）：事务标记 `failed`，旧实例
  未受任何影响，退出码 `1`。
- 停止后失败（含健康检查失败）：自动回滚——停止/注销新受管 unit、恢复
  `/etc/resolv.conf`、把旧 unit 文件放回原位（或 `unmask`）并按事务前
  enabled/active 状态重启，再清理新根内容；回滚成功返回 `5`，回滚失败返回 `6`。
- 前台实例场景无法自动重启旧实例，由用户负责。
- 中断恢复按事务阶段处理：`preparing` 标记 `failed`（迁移备份保留）；
  `prepared`/`stopping` 恢复旧 unit 与事务前 systemd 状态；
  `activating`/`verifying` 执行与失败相同的回滚。前台与 worker 之间的 `prepared`
  交接事务同样按此恢复（旧实例尚未停止，恢复为无操作）。

## 旧部署去留

迁移**不删除、不修改**旧部署目录和旧二进制；旧 unit 在成功后保持停止状态。
用户确认不再需要后自行清理。`lkit uninstall` 只管理 landscape 安装根目录，不涉及旧部署。
