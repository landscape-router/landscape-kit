# Docker 多发行版常用软件安装 E2E

## 目标

该测试在真实发行版容器里验证 `lkit software install docker` 的完整安装行为：发行版
检测、仓库文件写入、GPG key 下载与 dearmor、软件包管理器真实安装 docker-ce、服务
启用命令与 daemon 验证契约，以及安装后软件状态刷新。与换源 E2E 不同，本测试需要
联网下载 docker-ce 软件包与 GPG key。

覆盖发行版（每个发行版覆盖一个安装来源；TUNA 对 docker-ce 仓库存在地域/UA 过滤
（非 CN 流量 403），USTC 只同步 apt 家族（fedora 404），因此真实安装矩阵
使用官方源、阿里云与 USTC 源，TUNA、腾讯云与华为云的 URL 映射由单元测试
覆盖）：

| 容器镜像 | 家族 | 来源 | 覆盖点 |
| --- | --- | --- | --- |
| `debian:bookworm` | apt | official | `docker.list` 官方 URL、debian 代号、`docker.gpg` keyring 有效性 |
| `ubuntu:24.04` | apt | official | `docker.list` 官方 URL、ubuntu 代号、`docker.gpg` keyring 有效性、x86_64/aarch64（CI）两种架构的 `arch=` 映射 |
| `fedora:latest` | dnf | aliyun | `docker-ce.repo` 按 `VERSION_ID` 主版本生成 baseurl、`gpgkey` 指向镜像 |
| `archlinux:latest` | pacman | ustc | 官方仓库真实安装（来源参数被接受，pacman 不写第三方仓库） |

## 流程

`scripts/test-docker-software.sh`：

1. 用 `rust:1.97.1-bookworm`（固定 digest，与换源 E2E 一致）构建一次 lkit 生产
   二进制，写入 docker 命名卷 `lkit-software-bin`；
2. 对每个发行版运行 `docker run`：把二进制卷挂到 `/usr/local/bin`（只读），执行
   `scripts/docker/software/run-distro.sh <distro>`；
3. 汇总各发行版结果，任一失败整体返回非零。

容器内验证序列：

```text
software list（Docker 未安装）
    → software install docker --non-interactive（无 --source 报用法错误）
    → 注入 systemctl/docker 记录型 shim
    → software install docker --yes --source <来源>
    → 断言仓库文件 / keyring / 真实 /usr/bin/docker
    → 断言 systemctl.log 含 "enable --now docker"、docker.log 含 "info"
    → software list（Docker 已安装）
```

## 服务层 shim

容器内没有 systemd PID 1，也无法运行真实 dockerd（无特权），因此安装流程中的
`systemctl enable --now docker` 与最终验证 `docker info` 由记录型 shim 承担：调用
参数写入 `/var/log/lkit-software/*.log` 并成功返回，脚本断言调用契约（`enable
--now docker`、`info`）。真实服务启停与 daemon 运行属于宿主行为，不在此层重复；
其余流程（仓库文件、GPG key、软件包安装）全部真实执行。

Debian/Ubuntu 镜像自带的 `policy-rc.d` 会阻止 docker-ce postinst 真实启动服务，
不会与 shim 冲突。

## 环境适配

- 断言使用容器内真实的 `/etc/os-release`：apt 代号（`VERSION_CODENAME`）与 dnf 主
  版本号（`VERSION_ID` 取 `.` 前段）动态生成期望值；
- apt 架构按容器 `uname -m` 映射（x86_64 → `amd64`，aarch64 → `arm64`），与 lkit
  的 `std::env::consts::ARCH` 映射一致，CI 的 aarch64 runner 自动覆盖 arm64 分支；
- Fedora 的 docker-ce 仓库随官方发布节奏更新，`fedora:latest` 若在 Docker 官方
  尚未支持的版本上安装失败，把矩阵固定到 Docker 官方已支持的 Fedora 版本；
- pacman（Arch）不写第三方仓库，来源参数仅验证被接受。

## 本地运行

```sh
scripts/test-docker-software.sh
```

要求 Docker 与联网（下载 docker-ce 软件包与 GPG key）；默认仅支持 Linux x86_64
（CI 在原生 x86_64/aarch64 runner 上执行）。首次需要拉取发行版镜像与构建依赖。
