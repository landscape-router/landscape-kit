# `lkit software`

安装当前主机的常用软件（常用软件助手）。第一个（也是当前唯一的）软件是 Docker 容器
引擎；菜单与命令结构按软件列表设计，后续软件可直接扩展。

```text
lkit software list
lkit software install docker [--source official|aliyun|tuna|ustc] [--yes]
```

无参数且连接终端时进入交互选择：先列出软件（含安装状态）供选择，再选择安装来源并
确认后执行。`--non-interactive` 且无参数时报参数使用错误（退出码 `2`）。

## 软件列表与状态

`software list` 只读列出受管软件及安装状态（检测常见安装路径下是否存在可执行文件，
不做版本比较）：

```text
software: common software for Debian:
  - Docker (docker) [not installed]
```

## 安装来源

`--source` 可选 `official`（Docker 官方仓库）、`aliyun`（阿里云镜像）、`tuna`
（清华 TUNA 镜像）或 `ustc`（中科大 USTC 镜像）。未指定且交互时先选择来源；
`--non-interactive` 未指定来源属于参数使用错误。

## 支持范围

从 `/etc/os-release` 的 `ID` 读取发行版家族（与 `set-mirror` 相同的检测），按家族
安装：

| 家族 | 软件包管理器 | 安装方式 |
| --- | --- | --- |
| Debian、Ubuntu | apt | 写入 GPG key 到 `/etc/apt/keyrings/docker.gpg`、写 `/etc/apt/sources.list.d/docker.list`，`apt-get update` 后安装 `docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin` |
| Fedora、Rocky、AlmaLinux | dnf | 写 `/etc/yum.repos.d/docker-ce.repo`（baseurl 按家族与主版本号生成），安装同套 `docker-ce` 软件包 |
| Arch | pacman | 官方仓库直接安装 `docker docker-buildx docker-compose` |

不支持的其他发行版（如 Alpine）由发行版检测直接报错阻断；缺少 apt 代号或 dnf 主版本
号、不支持的 CPU 架构（apt 家族）同样报错。

安装完成后通过 `systemctl enable --now docker` 启用并启动服务，并以 `docker info`
做最终验证：daemon 未就绪时报"服务未运行"错误，安装视为失败。

## 权限与确认

安装与启动服务需要 root 权限（非 root 报错阻断，`software list` 只读不需要 root）。
交互模式默认在 `/dev/tty` 上二次确认；`--yes` 跳过确认。
