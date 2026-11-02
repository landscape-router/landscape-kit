# `lkit software`

安装当前主机的常用软件（常用软件助手）。受管条目分两类：常用软件与 Landscape 依赖的
基础系统包。

```text
lkit software list
lkit software install docker [--source official|aliyun|tencent|huawei|tuna|ustc] [--yes]
lkit software install base-packages [--packages ppp,iproute2,iw,hostapd,procps] [--yes]
```

无参数且连接终端时进入交互选择：先列出受管条目（含安装状态）供选择，再选择安装来源
（docker）或直接确认（base-packages）。`--non-interactive` 且无参数时报参数使用错误
（退出码 `2`）。

## 软件列表与状态

`software list` 只读列出受管条目及安装状态（按 PATH 探测可执行文件，不做版本比较）：

```text
software: common software for Debian:
  - Docker (docker) [not installed]
  - pppd (ppp) [not installed]
  - ip (iproute2) [installed]
  - iw (iw) [not installed]
  - hostapd (hostapd) [not installed]
  - sysctl (procps) [installed]
```

## 基础系统包

`base-packages` 安装 Landscape 运行依赖的基础系统包：`ppp`（pppd）、`iproute2`（ip）、
`iw`、`hostapd`、`procps`（sysctl）。`--packages` 限定子集（逗号分隔），缺省安装全部
缺失的包；已安装的包自动跳过。包管理器按 PATH 探测的 `apt-get`、`dnf`/`yum`、
`pacman`、`zypper` 或 `apk` 选择（包名按发行版映射，如 dnf 系的 `iproute`、`procps-ng`，
Alpine 的 `procps` 对应 `procps-ng`）。Alpine 执行 `apk add --no-cache <包名>`，
未识别到支持的包管理器时报错。apt 家族会先执行 `apt-get update` 刷新包列表
（精简镜像可能没有包索引），再执行安装。`--source` 仅适用于 docker，与
`base-packages` 同时使用属于参数使用错误。

Alpine 的基础依赖映射与检测如下：

| 条目 | apk 软件包 | 完整安装判定 |
| --- | --- | --- |
| `ppp` | `ppp ppp-pppoe` | `pppd` 可执行且 `/usr/lib/pppd/` 或其版本子目录有 `pppoe.so` / `rp-pppoe.so` |
| `iproute2` | `iproute2`（依赖 `iproute2-tc`） | 原生 `ip` 与 `tc`，不接受 BusyBox applet |
| `iw` | `iw` | `iw` 可执行 |
| `hostapd` | `hostapd` | `hostapd` 可执行 |
| `procps` | `procps-ng` | 原生 `sysctl`，不接受 BusyBox applet |

Alpine 3.22 的 `ppp` 元包依赖 `ppp-pppoe`；显式列出插件包也覆盖精简系统只安装
`ppp-daemon` 的修复路径。`software list`、控制台选中/置灰与实际安装共用完整性检测，
`lkit check` 对缺插件与不完整 iproute2 给出 apk 安装建议。其他包管理器保留按二进制
检测的语义。基础包安装不配置或启动无线热点，也不执行 PPPoE 拨号。

## 安装来源

`--source` 可选 `official`（Docker 官方仓库）、`aliyun`（阿里云镜像）、`tencent`
（腾讯云镜像）、`huawei`（华为云镜像）、`tuna`（清华 TUNA 镜像）或 `ustc`（中科大
USTC 镜像）。未指定且交互时先选择来源；`--non-interactive` 未指定来源属于参数
使用错误。

Alpine 只接受 `--source official`，表示从当前配置的 apk 仓库安装，而非 Docker CE
官方仓库。控制台仅展示“当前配置的 Alpine 仓库”；切换下载镜像使用 `lkit set-mirror`。
必须在 `/etc/apk/repositories` 中启用与系统版本一致的 `community` 仓库；安装器不自动
启用仓库、不切换到 edge，也不改写 apk 仓库配置。

## 支持范围

从 `/etc/os-release` 的 `ID` 读取发行版家族（与 `set-mirror` 相同的检测），按家族
安装：

| 家族 | 软件包管理器 | 安装方式 |
| --- | --- | --- |
| Debian、Ubuntu | apt | 写入 GPG key 到 `/etc/apt/keyrings/docker.gpg`、写 `/etc/apt/sources.list.d/docker.list`，`apt-get update` 后安装 `docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin` |
| Fedora、Rocky、AlmaLinux | dnf | 写 `/etc/yum.repos.d/docker-ce.repo`（baseurl 按家族与主版本号生成），安装同套 `docker-ce` 软件包 |
| Arch | pacman | 官方仓库直接安装 `docker docker-buildx docker-compose` |
| Alpine | apk | 当前系统仓库安装 `docker docker-openrc docker-cli-buildx docker-cli-compose`，使用 OpenRC 启用并启动 Docker |

不支持的其他发行版由发行版检测直接报错阻断；缺少 apt 代号或 dnf 主版本
号、不支持的 CPU 架构（apt 家族）同样报错。

Alpine 安装完成后执行 `rc-update add docker default` 与 `rc-service docker start`，
随后最多探测 30 次 `docker info`（失败后间隔 500ms），等待 OpenRC 后台启动的 daemon
就绪；等待支持取消，不把服务启动命令返回视为 daemon 已就绪。
其他家族通过 `systemctl enable --now docker` 启用并启动服务（找不到 systemctl 时回退
`service docker start`）。最后以 `docker info` 做最终验证：daemon 未就绪时报
"服务未运行"错误，安装视为失败。包管理器、OpenRC 启用或启动失败时立即停止，不报告成功。

## 权限与确认

安装与启动服务需要 root 权限（非 root 报错阻断，`software list` 只读不需要 root）。
交互模式默认在 `/dev/tty` 上二次确认；`--yes` 跳过确认。
