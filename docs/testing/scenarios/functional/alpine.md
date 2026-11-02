# Alpine / OpenRC 真实虚拟机

入口：[QEMU Alpine CI](../../../../.github/workflows/test-qemu-alpine.yml)、
[宿主脚本](../../../../scripts/test-qemu-alpine.sh)、
[guest 断言](../../../../scripts/qemu/alpine/guest-tests.sh)。

CI 用校验 SHA256 的 Alpine 3.22.2 x86_64 minirootfs，在容器内安装来自 v3.22 签名仓库的
OpenRC、linux-virt、SSH 和 Docker/Buildx/Compose，导出 rootfs 并用 `mke2fs -d` 生成磁盘。QEMU 必须使用 KVM；
管理 SSH 仅转发到宿主随机回环端口，guest 不可访问外网。lflare 使用 VM 内部的
namespace/veth，不创建宿主 TAP、桥或 bpffs 挂载。

测试运行普通 musl `lkit/lflare`，不传 `--test-runtime`。daemon 经生产 `self install`
注册；最小 Landscape 进程使用产品的 OpenRC renderer 生成服务定义，验证 init 启动顺序，
不验证真实 Landscape 的业务功能或 bpffs 就绪等待。

Alpine 的 sysfs 服务可能自行挂载 bpffs；测试专用 boot 服务在 VM 内移除该挂载，
确保 daemon 的挂载兜底被实际执行。测试不在本地正常开发中运行，要求显式 `LKIT_E2E=1`。
失败保留镜像构建日志、串口、OpenRC、daemon、客户端、Docker/containerd 和挂载诊断。
联网容器中的真实 apk 换源与软件安装场景见 [Alpine 测试说明](../../../testing/alpine.md)。

## ALP-01

**OpenRC 下通过 self install 部署 daemon 并实际挂载 bpffs**

- 测试层：QEMU/KVM + 真实 Alpine/OpenRC
- 状态：`已覆盖`
- 证据：[guest-tests.sh initial](../../../../scripts/qemu/alpine/guest-tests.sh)、
  [bpffs 启动策略](../../../service/runtime-and-health.md#daemon-启动与-bpffs)。
- 说明：先确认没有挂载，再安装 daemon，断言可执行服务原件、服务状态、实际 bpf/rw
  挂载、挂载成功日志、root-only daemon 日志，以及只读 check 的 bpffs 通过结果。

## ALP-02

**重复部署 daemon 不叠加 bpffs 挂载**

- 测试层：QEMU/KVM + 真实 Alpine/OpenRC
- 状态：`已覆盖`
- 证据：[guest-tests.sh initial](../../../../scripts/qemu/alpine/guest-tests.sh)。
- 说明：再次 self install 后 daemon PID 改变，挂载数量仍为一，挂载成功日志没有重复。

## ALP-03

**冲突或只读挂载不被修改，daemon、lflare 与 Landscape 启动不受阻断**

- 测试层：QEMU/KVM + VM 内 namespace/veth
- 状态：`已覆盖`
- 证据：[guest-tests.sh initial](../../../../scripts/qemu/alpine/guest-tests.sh)。
- 说明：分别构造只读 tmpfs 占用和只读 bpffs，断言错误诊断、原挂载类型/属性保持、
  daemon 存活；两种故障下 lflare 均完成真实 L2 握手并得到服务端认证日志，
  产品服务模板下的最小 Landscape 测试进程也能启动。

## ALP-04

**真实重启后 daemon 挂载 bpffs，Landscape 服务按 OpenRC 弱依赖顺序启动**

- 测试层：QEMU/KVM 重启、OpenRC 并行启动
- 状态：`已覆盖`
- 证据：[guest-tests.sh rebooted](../../../../scripts/qemu/alpine/guest-tests.sh)、
  [最小 Landscape 进程](../../../../scripts/qemu/alpine/router-fixture.c)、
  [生产模板生成器](../../../../scripts/qemu/alpine/render-services.rs)。
- 说明：宿主断言 boot ID 改变；boot fixture 先移除 bpffs，daemon 自动挂载，两个服务
  均启动且 lflare 可连接。Landscape fixture 仅检查 OpenRC 已标记 daemon 启动完成，
  不以 bpffs 成功作为启动条件。

## ALP-05

**self remove 幂等移除 OpenRC daemon，不卸载共享 bpffs**

- 测试层：QEMU/KVM + 真实 Alpine/OpenRC
- 状态：`已覆盖`
- 证据：[guest-tests.sh rebooted](../../../../scripts/qemu/alpine/guest-tests.sh)。
- 说明：连续移除两次均成功，服务链接消失，CLI 二进制保留，bpffs 仍为可读写挂载。

## ALP-06

**真实 Docker OpenRC 服务启停、重启自启动及 CLI 插件**

- 测试层：QEMU/KVM + 真实 Alpine/OpenRC/Docker
- 状态：`待 CI 验证`
- 证据：[guest-tests.sh](../../../../scripts/qemu/alpine/guest-tests.sh)、
  [rootfs Docker 软件包](../../../../scripts/qemu/alpine/Dockerfile)、
  [Alpine CI](../../../../.github/workflows/test-qemu-alpine.yml)。
- 说明：构建 rootfs 时安装 v3.22 main/community 签名软件包；VM 真实执行
  `rc-update add docker default`、`rc-service docker start/stop`、`docker info`，
  验证 default runlevel 链接、重启后服务运行及 Buildx/Compose 版本命令。
  不拉取容器镜像，所有 cgroup、存储和 Docker 网络操作仅在 VM 中进行。
- 缺口：不同 Alpine 版本与非 x86_64 架构未纳入此真实系统矩阵。
