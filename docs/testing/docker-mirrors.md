# Docker 多发行版换源 E2E

## 目标

该测试在真实发行版容器里验证 `lkit set-mirror`（换源）的完整行为：检测发行版、
切换镜像、备份、恢复，以及"仅 CD 源/无默认源"时的兜底。与生命周期 E2E 不同，换源
只读写本机软件源文件、不联网、不依赖 systemd，因此直接用各发行版的官方基础镜像，
容器内以 root 运行生产二进制，无需 `test-support`。

覆盖发行版：

| 容器镜像 | 家族 | 覆盖点 |
| --- | --- | --- |
| `debian:bookworm` | apt（deb822，新版镜像无 `sources.list`） | one-line 与 deb822 两种布局、Debian security 默认保留官方、`--replace-security`、CD 源默认注释+合成与 `--keep-cdrom` 转换两种兜底 |
| `ubuntu:24.04` | apt（deb822） | Ubuntu security 并入主仓库路径、x86_64 与 aarch64（CI）两种架构下 CD 源兜底（`/ubuntu` vs `/ubuntu-ports`、`archive.ubuntu.com` vs `ports.ubuntu.com`） |
| `fedora:latest` | dnf | `#baseurl=` 解注释与重写、metalink 注释、fedora/epel 映射 |
| `archlinux:latest` | pacman | mirrorlist 整体重新生成（单 Server） |

## 流程

`scripts/test-docker-mirrors.sh`：

1. 用 `rust:1.97.1-bookworm`（固定 digest，与生命周期 E2E 一致）构建一次 lkit
   生产二进制，写入 docker 命名卷 `lkit-mirror-bin`（命名卷避免 docker 对缺失
   宿主路径的自动创建）；
2. 对每个发行版运行 `docker run`：把二进制卷挂到 `/usr/local/bin`（只读），执行
   `scripts/docker-mirrors/run-distro.sh <distro>`；
3. 汇总各发行版结果，任一失败整体返回非零。

容器内验证序列（各家族相同）：

```text
官方源 → set-mirror tuna → 断言镜像 URL 生效
      → set-mirror --restore → 断言恢复原内容、备份删除
      → set-mirror aliyun → 断言镜像互转
      → set-mirror official → 断言恢复官方主机
```

Debian 与 Ubuntu 额外执行"仅 CD 源"场景（Ubuntu 同时覆盖"空源文件"与 `--check` 由
Debian 覆盖）：清空受管文件，只留一行 `deb cdrom:[...]`，`set-mirror tuna` 默认应
注释掉该行并合成镜像条目（避免系统无可用源），`--restore` 还原 cdrom 行；再用
`set-mirror tuna --keep-cdrom` 验证转换路径（cdrom 行转换为镜像、保留
suites/components）；随后把 `sources.list` 置空，`set-mirror tuna` 应合成完整的新
条目（提示 "added a new Debian source entry"）；最后写入一条无法识别的行，
`set-mirror --check` 应非零退出并指出行号，干净文件退出 0。

## 环境适配

- Fedora 镜像的 `.repo` 使用 `download.example` 占位主机，脚本先用 `sed` 换为规范
  官方主机（`download.fedoraproject.org/pub/`），保证验证的是"官方源 → 镜像"映射；
  另补一个 `epel.repo` fixture 覆盖 epel 路径；
- Debian 镜像布局随版本变化（老版本 one-line `sources.list`，新版本 deb822
  `sources.list.d/debian.sources`），断言对两种布局都兼容；
- Ubuntu 的断言按容器架构选择：x86_64 期望 `archive.ubuntu.com` + `/ubuntu`，
  arm64 等 ports 架构期望 `ports.ubuntu.com` + `/ubuntu-ports`（CI 的 aarch64
  runner 上自动覆盖该分支）；
- 镜像 URL 保留原始协议（`http://`/`https://`），断言不依赖 scheme。

## 本地运行

```sh
scripts/test-docker-mirrors.sh
```

要求 Docker；默认仅支持 Linux x86_64（CI 在原生 x86_64/aarch64 runner 上执行）。
运行时无需网络（换源不联网），但首次需要拉取发行版镜像与构建依赖。
