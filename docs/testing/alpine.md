# Alpine 换源、软件包与 OpenRC 测试

入口为 [Alpine CI](../../.github/workflows/test-qemu-alpine.yml)。同一任务构建普通 musl
生产二进制，分别在联网的临时 Alpine 容器与不可访问外网的 QEMU/KVM 虚拟机中验证。
不使用 `--test-runtime`，不在本地正常开发流程中运行系统级测试。

## apk 与换源容器

[`scripts/docker/alpine/run-tests.sh`](../../scripts/docker/alpine/run-tests.sh) 只允许在
显式 `LKIT_E2E=1`、root、存在 `/.dockerenv` 和 `/etc/alpine-release` 的容器运行。
CI 使用 `alpine:3.22`，只读挂载本次构建的 `lkit-x86_64` 和测试脚本，不挂载宿主系统
配置目录、不使用 host 网络或 privileged 模式。

真实执行的部分：

- 发行版检测、软件列表、拒绝 Docker CE 来源参数。
- 缺少 community 仓库时 apk 安装失败，且不启动服务。
- 官方源 → 阿里云 Alpine 源、真实 `apk update`、源文件权限、备份、重复换源保留备份、
  逐字节恢复和官方反向映射；禁用的 edge/testing 行保持原样。
- BusyBox `ip`/`sysctl` 不被判定为完整依赖；安装真正的 `iproute2`、`procps-ng`。
- 从只有 `ppp-daemon` 的状态修复 PPPoE 插件；检查命令、插件文件和包安装状态。
- 安装 `ppp ppp-pppoe iproute2 iw hostapd procps-ng` 及 Docker、OpenRC 服务脚本、Buildx、
  Compose，确认下载遵循当前 apk 镜像且不写入 apt/dnf Docker CE 源。

容器内的 `rc-update`、`rc-service` 和 `docker info` 使用记录型 shim，断言启用、启动、
最终验证的顺序，不在共享宿主内启动 Docker daemon。真实包安装与文件系统操作仅发生
在一次性容器内。国内镜像网络不可用或真实包安装失败会使 CI 失败，不跳过该断言。
容器日志保存为 `alpine-artifacts/apk.log`。

## 真实 OpenRC 与 Docker daemon

[QEMU guest 脚本](../../scripts/qemu/alpine/guest-tests.sh) 在真实 Alpine/OpenRC 下检查
Docker 服务的启用、启动、停止、重启后自启动、`docker info`，以及 `docker buildx version`
和 `docker compose version`。软件包在构建 rootfs 时从 v3.22 main/community 签名仓库
安装；VM 不访问外网、不拉取容器镜像。cgroups 在 boot runlevel 挂载，Docker 的网络和
内核操作只发生在 VM 内，不接触 CI 宿主网络。

VM 同时执行 [ALP-01 至 ALP-06](scenarios/functional/alpine.md) 的 bpffs、daemon、flare
及 OpenRC 场景；保留串口、OpenRC、Docker/containerd 和进程诊断。

## 单元测试与边界

- 基础依赖检测使用临时目录模拟原生二进制、BusyBox 链接与版本化 PPPoE 插件。
- Docker 命令计划、失败即停止、就绪重试上限及取消使用闭包替身，不生成真实进程或等待。
- repositories 重写、备份恢复及回滚只操作临时目录。

容器和 VM 定义由 CI 执行，不以本地静态检查或单元测试通过代替真实场景通过。
当前真实系统矩阵固定 Alpine 3.22/x86_64；其他版本/架构、真实 PPPoE 链路、无线驱动、
固件和 AP 模式不由这组测试证明。
