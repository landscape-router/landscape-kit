# Landscape Kit

[English](README.en.md) | 中文

[Landscape](https://github.com/ThisSeanZhang/landscape) 是一个用 Rust + eBPF 构建的 Linux 路由器系统，提供 Web 管理界面和网络配置能力。**Landscape Kit (`lkit`)** 是它的本机管理工具——在路由器主机上运行，帮你完成安装部署、日常管理、故障诊断和后续升级。

适用场景：首次部署 Landscape、Web 界面不可用时的离线管理、批量配置和自动化运维。

---

## 功能

### 安装部署

```bash
# 交互式安装——引导你完成网络配置、源选择、下载和初始化
sudo lkit install

# 非交互安装——适合批量部署或脚本化
sudo lkit install --source github-default --version v0.19.2
```

自动检测系统架构和 libc 类型，从多个源（GitHub / HTTP 镜像 / S3 / 本地）中选择最快的下载，校验 SHA-256 后安装为 systemd 服务。

### 日常管理

```bash
lkit status              # 查看服务状态
lkit service restart     # 重启服务
lkit logs -n 100         # 查看最近 100 行日志
lkit diagnose            # 系统健康检查（磁盘、API、systemd、端口等）
```

不带参数运行 `lkit` 会进入交互式菜单，所有操作都可以通过选择完成。

### 镜像管理

```bash
# 从 GitHub 或 HTTP 镜像同步制品到本地
lkit mirror sync --target local --path /data/mirror --latest 5

# 同步到 S3/R2 存储
lkit mirror sync --target s3 --bucket my-bucket --endpoint https://s3.example.com

# 验证已同步的镜像
lkit mirror verify --target local --path /data/mirror

# 启动本地 HTTP 镜像服务
lkit mirror serve --path /data/mirror --port 8080
```

镜像管理适合在内网环境搭建私有制品源，或者为多台路由器提供本地下载。

### 完整命令列表

| 命令 | 说明 |
|------|------|
| `lkit` | 交互式主菜单 |
| `lkit status [--json]` | 服务状态 |
| `lkit service start\|stop\|restart` | 服务控制 |
| `lkit logs [-n N]` | 日志查看 |
| `lkit diagnose [--json]` | 系统诊断 |
| `lkit install` | 安装/初始化 Landscape |
| `lkit mirror sync\|serve\|verify\|list` | 制品镜像管理 |
| `lkit self version` | lkit 版本信息 |
| `lkit backup` | 备份管理（计划中） |
| `lkit upgrade` | 升级（计划中） |
| `lkit rollback` | 回滚（计划中） |

## 配置

### 路径

| 项目 | 默认值 | 覆盖方式 |
|------|--------|----------|
| Landscape 数据目录 | `~/.landscape-router` | `LANDSCAPE_HOME` 环境变量 |
| lkit 配置目录 | `~/.landscape-kit/` | `LKIT_HOME` 环境变量 |
| 自定义下载源 | `~/.landscape-kit/config/lkit.toml` | 编辑文件 |

### 自定义下载源

在 `lkit.toml` 中添加额外的制品源，lkit 会在安装时自动探测所有可用源并选择最优：

```toml
[[sources]]
name = "内网镜像"
type = "http"
url = "https://mirror.example.com/landscape"
priority = 1

[[sources]]
name = "私有 S3"
type = "s3"
bucket = "landscape-releases"
endpoint = "https://s3.example.com"
region = "us-east-1"
priority = 5
```

### 日志级别

```bash
lkit -v status      # INFO
lkit -vv status     # DEBUG
RUST_LOG=debug lkit # 环境变量
```

## 参与开发

```bash
# 提交前验证
cargo fmt --all && cargo clippy --all -- -D warnings && cargo test --workspace
```

- 贡献流程：[CONTRIBUTING.md](CONTRIBUTING.md)
- 编码约定：[docs/CONVENTIONS.md](docs/CONVENTIONS.md)
- 设计规格：[docs/spec/](docs/spec/)

## 路线图

| 里程碑 | 说明 | 状态 |
|--------|------|------|
| M1 | CLI 骨架、交互菜单、服务管理、日志、诊断 | 已完成 |
| M2 | 安装部署（Wizard + systemd + 网络配置） | 已完成 |
| M2.5 | 多源下载、镜像管理工具 | 已完成 |
| M3 | 备份恢复、升级回滚 | 计划中 |

## 许可证

[AGPL-3.0](LICENSE)
