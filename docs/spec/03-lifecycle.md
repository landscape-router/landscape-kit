# 安装、初始化与生命周期

## 1. 文档信息

- 状态：Draft
- 项目名：Landscape Kit

## 2. 路径发现规则

### 2.1 Landscape HOME 发现

Landscape HOME 的发现顺序定义如下：

1. 管理器 CLI 显式传入的 HOME 参数
2. 环境变量 `LANDSCAPE_HOME`
3. 从运行中的 Landscape 进程 / 启动参数中解析配置目录
4. 默认路径 `~/.landscape-router`
5. 以上都失败，则判定为未安装或不可判定

### 2.2 管理器工作目录发现

管理器自身工作目录的发现顺序：

1. 环境变量 `LKIT_HOME`
2. 默认路径 `~/.landscape-kit/`

内部结构：

- `runtime/`：管理器托管运行目录
- `tmp/`：临时目录 / staging 区
- `backup/`：备份仓库
- `config/`：管理器自身配置与索引

### 2.3 重要约束

- **二进制路径发现** 与 **HOME 发现** 必须分开设计
- "通过进程拿到可执行文件路径" 不等于 "确定 HOME 路径"
- Landscape HOME 与管理器工作目录必须彻底分离

## 3. 权限与用户

### 3.1 需要 root / sudo 的操作

- 写入 `/etc/systemd/system/`（systemd unit 安装）
- `systemctl daemon-reload` / `enable` / `start`
- 写入 Landscape HOME（若 HOME 属主为专用用户而非当前用户）

### 3.2 普通用户可执行的操作

- `lkit status`
- `lkit logs`
- `lkit diagnose`
- `lkit config export`
- `lkit backup create/list/restore`（需对 Landscape HOME 有读权限）
- `lkit upgrade check`
- `lkit self version` / `lkit self upgrade check`

### 3.3 权限不足时的处理

- 命令执行前做权限预检
- 权限不足时输出明确的错误信息和建议的提权方式
- 不尝试自动提权（不隐式调用 sudo）

## 4. 目录模型

### 4.1 backup 索引

管理器本地维护 `backup.json` 作为 CLI 展示索引（非持久化缓存，重建不影响数据完整性）：

- **仅为展示索引**，用于 `lkit backup list` 加速展示
- **不是备份真相来源**
- 真实备份信息以备份包中的 backup manifest 与 frozen backup index 为准
- 可从备份目录重新扫描重建

Landscape 维护的 `landscape_backup_index.json` 是备份范围的权威声明，管理器只负责读取与执行。两者是不同文件，不同职责。

## 5. Landscape 初始化机制

### 5.1 `landscape_init.toml` 的职责

`landscape_init.toml` 是 **初始化输入文件**，不是 Landscape 的长期运行时真相源。

其特点：

- 只在 **首次初始化 / 重初始化** 时需要
- 初始化后，其内容会被拆分并落到：
  - `landscape.toml`
  - `landscape_db.sqlite`
- 后续正常运行时，Landscape 主要依赖：
  - `landscape.toml`
  - `landscape_db.sqlite`
  - `landscape_init.lock`

### 5.2 `landscape_init.lock` 的职责

`landscape_init.lock` 是关键控制文件：

- 存在：跳过初始化
- 不存在：触发初始化
  - 若存在 `landscape_init.toml`，则按其内容初始化
  - 若不存在 `landscape_init.toml`，则清空已有配置后使用默认初始化逻辑

### 5.3 对产品设计的影响

因此需要明确区分两种动作：

1. **实例恢复**：恢复一个已安装实例的当前状态
2. **按配置重建**：通过导出的 `landscape_init.toml` 重新部署/重建实例

## 6. 安装与初始化设计

### 6.1 V1 目标

管理器首版需要支持：

- 无参数启动进入通用启动器，可导航至安装/备份/状态等
- 自动初始化 Landscape
- 在引导流程中完成最小闭环的基础网络部署
- 无网络情况下的本机安装/救援入口

### 6.2 安装入口

V1 定义两类安装入口：

1. **通用启动器引导**
   - `lkit`（无参数）进入通用启动器，安装是其中一个入口
   - 面向人工部署、现场安装、救援场景

2. **文件驱动安装**
   - `lkit install --init-file <file>`
   - 面向自动化部署、配置重建、非交互安装

### 6.3 已安装实例再次执行 `install` 的策略

V1 默认行为：检测到已安装实例时拒绝执行，提示用户：
- 使用 `lkit repair`（保留数据修复安装）— V2 实现
- 使用 `--force` 覆盖安装（会先创建自动备份点）

### 6.4 release source 模型

采用 **多源并发探测** 架构，通过 `ReleaseSource` trait 统一抽象所有源类型。详见 [09-release-source.md](./09-release-source.md)。

支持的源类型：

- GitHub Releases
- HTTP(S) 镜像源（含 Cloudflare R2 公开桶）
- 本地文件路径 / `file://` 路径

优先级（三级）：

1. CLI 显式指定的 source
2. `lkit.toml` 中声明的 `[[sources]]` 列表
3. 内置默认 GitHub Releases

同优先级的源并发 HEAD 探测，选延迟最低的。实际下载失败时自动 fallback 到次优源。

### 6.5 release artifact 约定

V1 安装/升级流程应将 release 视为完整制品集合，至少包含：

- Landscape 后端二进制（`landscape-webserver-{arch}`）
- 辅助二进制（`redirect_pkg_handler-{arch}`）
- `static.zip`
- `release-manifest.json`（制品清单，推荐；由 `lkit mirror sync` 生成）
- `SHASUM256sum.txt`（校验和文件，上游提供）

其中：

- `static.zip` 作为 V1 固定静态资源包格式，与 Landscape 官方 release 保持一致
- 安装与升级流程必须同时处理 binary 与 `static.zip`
- 优先从 `release-manifest.json` 获取制品列表和校验和，fallback 到 `SHASUM256sum.txt`
- `release-manifest.json` 不存在时不再拒绝操作，而是降级使用 `SHASUM256sum.txt`

### 6.6 systemd 安装方式

V1 服务安装方式仅支持 **systemd**。

Unit 规格：

- Unit 名称：`landscape.service`
- 安装路径：`/etc/systemd/system/`
- 运行用户：root（V1 默认）
- 参数传递：通过 `ExecStart` 显式传参（`--home`、`--web-root`）

安装流程：

1. 创建 HOME
2. 安装 binary
3. 解压 `static.zip`
4. 写入初始化输入
5. 生成 systemd unit
6. `daemon-reload`
7. `enable`
8. `start`
9. health check

### 6.7 安装状态机

将安装流程抽象为统一状态机：

1. Precheck
2. Collect Config
3. Resolve Release
4. Fetch Artifacts
5. Apply Host Changes
6. First Boot
7. Finalize

无论来源是引导式交互还是文件驱动安装，最终都落到统一应用层执行。

### 6.8 失败处理原则

- Precheck 失败：直接退出，不落地安装结果
- Fetch 失败：不修改系统运行态
- Apply 失败：尽量回滚本次写入，不覆盖已有可用安装
- First Boot 失败：保留诊断信息，不宣称安装成功

## 7. 管理器自身更新

### 7.1 V1 目标

管理器自身支持更新，但仅做 **手动触发、受控更新**。V1 只实现 `check`（检查提示），`apply` 留到后续版本。

### 7.2 用户动作

- `lkit self version`
- `lkit self upgrade check`
- `lkit self upgrade apply`（V2）

### 7.3 策略

- 若发行方式是系统包（deb/rpm/apk），优先复用包管理器
- 若发行方式是单二进制，再实现内建自更新
- `lkit self upgrade` 作为统一入口，底层可适配不同安装方式

### 7.4 V1 非目标

- 不做后台静默自动更新
- 不做常驻自动检查服务

## 8. 并发与崩溃恢复

- **并发防护**：`lkit` 启动时通过 pidfile（`{manager_home}/runtime/lkit.pid`）检测已有实例，若已运行则拒绝执行（备份/升级/安装等写操作不可并发）
- **崩溃恢复**：staging 模式下，崩溃后残留的 tmp 文件不影响系统状态；下次启动时清理 `tmp/` 目录
- **原子写入**：关键文件（备份包、配置导出）使用 `write-tmp + fsync + rename` 保证崩溃不产生损坏文件

## 9. Landscape 与管理器的交互方式

### 9.1 V1 原则

首版不做 "Landscape 主动 push 控制 CLI" 的模型。

### 9.2 推荐模型

- 管理器主动调用 Landscape API 获取状态
- 管理器在本机执行：
  - 升级
  - 重启
  - 备份
  - 恢复
