# 常用软件安装场景

`lkit software` 安装当前主机的常用软件：检测发行版与安装状态，按家族配置 Docker 官方
或国内镜像仓库并安装 `docker-ce`，启用并启动服务后验证 daemon。控制台 Software
面板与命令共享同一安装流程。

## SFT-01

**Docker 官方源安装：apt 源文件与 dnf 仓库文件生成**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`software::docker` 测试](../../../../lkit-cli/src/software/docker.rs)
- 说明：`test-support` 下注入临时路径，断言 apt 的 `docker.list` 按
  `deb [arch=amd64 signed-by=<keyrings>/docker.gpg] <base>/linux/<family> <codename> stable`
  写入官方 URL（Debian bookworm / Ubuntu jammy），dnf 的 `docker-ce.repo` 按
  `<base>/linux/<slug>/<主版本>/$basearch/stable` 与 `gpgkey=<base>/linux/<slug>/gpg`
  写入（Rocky 9）；缺 apt 代号或缺 dnf 版本号时报错阻断；架构映射支持
  amd64/arm64/armhf。

## SFT-02

**镜像源安装：apt/dnf 仓库文件指向所选镜像**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`software::docker` 测试](../../../../lkit-cli/src/software/docker.rs)
- 说明：TUNA（Ubuntu jammy）、腾讯云与华为云（Ubuntu noble）与阿里云（Rocky 主
  版本号从 `9.3` 取 `9`）写入的仓库文件 URL 指向对应镜像主机，其余字段不变。

## SFT-03

**已安装状态检测与重复安装提示**

- 测试层：Rust 单元 + Rust 控制台测试
- 状态：`已覆盖`
- 证据：[`software::docker::docker_installed_detects_existing_binary` 测试](../../../../lkit-cli/src/software/docker.rs)、
  [`console::tests::software`](../../../../lkit-cli/src/console/tests/software.rs)
- 说明：`Software::installed` 检测常见安装路径下的 `docker` 可执行文件（注入临时
  路径验证存在/删除切换）；控制台对已安装软件按 Enter 只提示“已安装”并拒绝打开
  确认层；`lkit software install docker` 对已安装软件直接提示并失败退出。

## SFT-04

**发行版检测失败与非 root 阻断**

- 测试层：Rust 控制台测试
- 状态：`已覆盖`
- 证据：[`console::tests::software`](../../../../lkit-cli/src/console/tests/software.rs)
- 说明：面板检测失败时显示错误且确认 Enter 不启动安装（无 worker 产生）；非 root
  （`test-support` 下注入 `allow_non_root=false`）确认 Enter 报权限错误、不启动安装；
  面板渲染显示发行版摘要与软件行。CLI 非 root 安装报错阻断、
  `--non-interactive` 无参数报参数使用错误。

## SFT-05

**控制台 Software 面板交互**

- 测试层：Rust 控制台测试
- 状态：`已覆盖`
- 证据：[`console::tests::software`](../../../../lkit-cli/src/console/tests/software.rs)
- 说明：菜单导航（未安装/已安装均可到达 Software）、面板渲染（主机摘要、Docker 行与
  状态）、行选择边界（单行钳制）、确认层打开/关闭、来源循环切换（Space/Right 前进、
  Left 后退，Official→阿里云→腾讯云→华为云→TUNA→USTC 闭环）、确认层渲染（来源行
  高亮与切换提示文案）与安装进度弹窗渲染（阶段文案与 Gauge）。CLI 解析测试覆盖
  `software list`、`software install docker [--source ...]`、未知软件与非法来源拒绝、
  裸命令交互模式。

## SFT-06

**真实发行版容器内的 Docker 安装 E2E**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[`scripts/test-docker-software.sh`](../../../../scripts/test-docker-software.sh)、
  [`run-distro.sh`](../../../../scripts/docker/software/run-distro.sh)、
  [测试说明](../../../testing/docker-software.md)
- 说明：用 rust:bookworm 构建生产二进制，经 docker 命名卷挂载进
  `debian:bookworm`/`ubuntu:24.04`/`fedora:latest`/`archlinux:latest` 容器（root，无需
  test-support）执行 `lkit software install docker --yes`，每个发行版覆盖一个来源
  （debian=official、ubuntu=official、fedora=aliyun、arch=ustc；CI runner 在海外，
  ubuntu 使用官方源，国内镜像的 URL 映射由单元测试覆盖）：
  断言 apt `docker.list` 按真实 `VERSION_CODENAME` 与架构写入官方/镜像 URL、
  `docker.gpg` 为有效 keyring、dnf `docker-ce.repo` 按真实 `VERSION_ID` 主版本生成
  baseurl 与 gpgkey 指向阿里云、pacman 真实安装且来源参数被接受；安装后断言真实
  `/usr/bin/docker` 存在、`software list` 状态刷新为已安装；容器内无法运行
  dockerd，`systemctl enable --now docker` 与 `docker info` 用记录型 shim
  验证调用契约；安装前验证 `--non-interactive` 无 `--source` 报用法错误。CI 在
  x86_64 与 aarch64 runner 上运行，aarch64 自动覆盖 arm64 仓库架构分支。
- 缺口：TUNA 的 docker-ce 仓库存在地域/UA 过滤（非 CN 流量 403）、USTC 只同步
  apt 家族（fedora 404），未纳入真实安装矩阵（URL 映射由单元测试覆盖）；
  Rocky、AlmaLinux 尚未纳入容器矩阵；真实 dockerd 启动由宿主
  验收负责。
