# Docker 多发行版换源 E2E

## 目标

该测试在真实发行版容器里验证 `lkit set-mirror`（换源）的完整行为：检测发行版、
切换镜像、备份、恢复，以及"仅 CD 源/无默认源"时的兜底。与生命周期 E2E 不同，换源
改写软件源文件、不依赖 systemd，因此直接用各发行版的官方基础镜像，
容器内以 root 运行生产二进制，无需 `test-support`。

## 联网触发边界

本工作流只由推送到 `ci-mirror` 分支触发，不监听普通分支、PR、定时任务或手动 dispatch。
普通 CI 使用离线单测验证镜像 URL、文件改写和备份恢复。工作流串行执行两个架构，
完成后调用 Alpine 镜像实测；同分支的新推送取消旧任务。

宿主入口与容器入口均调用共享 guard，要求 `LKIT_ALLOW_MIRROR_NETWORK=1`、
`GITHUB_ACTIONS=true`、`GITHUB_EVENT_NAME=push`、`GITHUB_REF=refs/heads/ci-mirror`
同时成立，否则在 Docker、探测或包管理操作前失败。详情见[联网策略](mirror-network-policy.md)。

覆盖发行版：

| 容器镜像 | 家族 | 覆盖点 |
| --- | --- | --- |
| `debian:bookworm` | apt（deb822，新版镜像无 `sources.list`） | one-line 与 deb822 两种布局、Debian security 默认保留官方、`--replace-security`、CD 源默认注释+合成与 `--keep-cdrom` 转换两种兜底 |
| `ubuntu:24.04` | apt（deb822） | Ubuntu security 并入主仓库路径、x86_64 与 aarch64（CI）两种架构下 CD 源兜底（`/ubuntu` vs `/ubuntu-ports`、`archive.ubuntu.com` vs `ports.ubuntu.com`） |
| `fedora:44` | dnf | 完整镜像互转、原生占位地址解析、`#baseurl=` 解注释、metalink 注释、fedora/epel 映射 |
| `fedora:45` | DNF5 | vendor 与管理员目录、原生官方源转换及恢复、镜像可用性／拒绝零修改契约 |
| `archlinux:latest` | pacman | mirrorlist 整体重新生成（单 Server） |

## 流程

`scripts/test-docker-mirrors.sh`：

1. 用 `rust:1.97.1-bookworm`（固定 digest，与生命周期 E2E 一致）构建一次 lkit
   生产二进制，写入 docker 命名卷 `lkit-mirror-bin`（命名卷避免 docker 对缺失
   宿主路径的自动创建）；
2. 对每个发行版运行 `docker run`：把二进制卷挂到 `/usr/local/bin`（只读），执行
   `scripts/docker/mirrors/run-distro.sh <distro>`；
3. 汇总各发行版结果，任一失败整体返回非零。

常规验证序列（Fedora 44 与其他家族）：

```text
官方源 → set-mirror tuna → 断言镜像 URL 生效
      → set-mirror --restore → 断言恢复原内容、备份删除
      → set-mirror aliyun → 断言镜像互转
      → set-mirror official → 断言恢复官方主机
```

Fedora 45 先验证原生 vendor 配置转换到规范官方 URL、备份和逐文件还原。随后尝试
TUNA：成功时验证镜像 URL、备份和恢复；若明确提示仓库未同步，则断言该拒绝原因、
原文件逐字节不变且没有备份副作用。其他失败仍使测试失败。不关闭可用性探测，
不把新版公共镜像尚未发布当作产品应强制改写的理由。

Debian 与 Ubuntu 额外执行"仅 CD 源"场景（Ubuntu 同时覆盖"空源文件"与 `--check` 由
Debian 覆盖）：清空受管文件，只留一行 `deb cdrom:[...]`，`set-mirror tuna` 默认应
注释掉该行并合成镜像条目（避免系统无可用源），`--restore` 还原 cdrom 行；再用
`set-mirror tuna --keep-cdrom` 验证转换路径（cdrom 行转换为镜像、保留
suites/components）；随后把 `sources.list` 置空，`set-mirror tuna` 应合成完整的新
条目（提示 "added a new Debian source entry"）；最后写入一条无法识别的行，
`set-mirror --check` 应非零退出并指出行号，干净文件退出 0。

## 环境适配

- Fedora 44 的 `/etc/yum.repos.d/` 和 Fedora 45 的 `/usr/share/dnf5/repos.d/`
  均使用未经预处理的原生 `.repo`；`download.example` 必须由产品解析，切换后不允许
  残留占位地址。`/etc/yum.repos.d/epel.repo` fixture 覆盖管理员目录与 EPEL 路径，
  原生仓库与 fixture 均验证备份恢复的原始内容；
- Debian 镜像布局随版本变化（老版本 one-line `sources.list`，新版本 deb822
  `sources.list.d/debian.sources`），断言对两种布局都兼容；
- Ubuntu 的断言按容器架构选择：x86_64 期望 `archive.ubuntu.com` + `/ubuntu`，
  arm64 等 ports 架构期望 `ports.ubuntu.com` + `/ubuntu-ports`（CI 的 aarch64
  runner 上自动覆盖该分支）；
- 镜像 URL 保留原始协议（`http://`/`https://`），断言不依赖 scheme。

## 运行方式

审阅待测提交后，将其推送到专用分支：

```sh
git push origin HEAD:ci-mirror
```

不在本地执行系统级脚本，也不通过伪造 CI 环境变量绕过 guard。CI 在原生 x86_64/aarch64
runner 上使用 Docker，需要网络拉取镜像、构建依赖并执行软件包索引刷新。离线回归只
操作临时目录，不向公共镜像站发送请求。
