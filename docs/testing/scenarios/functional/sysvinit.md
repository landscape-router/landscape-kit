# Debian / SysVinit 真实虚拟机

入口：[SysVinit QEMU CI](../../../../.github/workflows/test-qemu-sysvinit.yml)、
[宿主脚本](../../../../scripts/test-qemu-sysvinit.sh)、
[guest 断言](../../../../scripts/qemu/sysvinit/guest-tests.sh)。

测试通过校验 SHA256 的 Debian archive keyring 验证 trixie 签名仓库，用 `mmdebstrap`
构建包含 `sysvinit-core`、`sysv-rc`、内核、OpenSSH 与 ifupdown 的 amd64 rootfs。
虚拟机运行生产 `lkit`，不使用 `--test-runtime`、fake service manager 或 Landscape fixture。
管理网络为固定 guest IPv4；此入口不部署 Landscape，不执行网络接管。

QEMU 必须使用 KVM；管理 SSH 只转发到宿主随机回环端口，`restrict=on` 阻止 guest
访问外网，不修改宿主网卡。入口要求 root 和显式 `LKIT_E2E=1`，guest 断言还检查专用
标记文件与主机名。测试由 CI 执行，不属于提交前单元测试；专用测试环境可通过
`LKIT_QEMU_PREBUILT` 指定生产二进制、`LKIT_QEMU_ARTIFACT_DIR` 指定持久产物目录。

三阶段日志为 `initial.log`、`enabled-reboot.log`、`disabled-reboot.log`，每次断言写入
`results.tsv`。CI 同时归档串口、镜像构建日志、`guest-diagnostics.log`、`guest-logs.tar.gz`、
`guest-packages.tsv` 和 `lkit.sha256`。实际发行版、内核、PID 1 与 lkit 版本随 guest 日志保存。
`lkit check` 只断言后端识别行，不要求整个 Landscape 宿主预检通过。

这些场景证明 Debian trixie 的 SysVinit daemon 生命周期，不代表 Devuan 等其他发行版、
其他架构或 SysVinit 完整网络接管已验收。跨后端范围见
[QEMU 服务管理后端验收](../../qemu-service-managers.md)。

## SVI-01

**真实 SysVinit 下自动选择后端并安装、注册和启动 daemon**

- 测试层：QEMU/KVM + Debian trixie/SysVinit
- 状态：`已覆盖`
- 证据：[guest-tests.sh assert_identity / initial](../../../../scripts/qemu/sysvinit/guest-tests.sh)。
- 说明：检查 PID 1 为 `/sbin/init` 对应进程、`sysvinit-core` 已安装、没有 systemd 运行
  目录或 OpenRC 执行入口；`lkit check` 报告 SysVinit。`self install` 生成可执行服务原件及
  `/etc/init.d/lkit.service` 链接，创建 S 启动链接并拉起服务。实际进程可执行路径与
  参数匹配生产 lkit，daemon 的父进程为 supervisor，四个能力标记指向当前 daemon PID。

## SVI-02

**daemon 子进程崩溃后由同一监管进程拉起**

- 测试层：QEMU/KVM + 真实 SysVinit 服务
- 状态：`已覆盖`
- 证据：[guest-tests.sh initial](../../../../scripts/qemu/sysvinit/guest-tests.sh)、
  [退避策略场景 DAE-13](daemon.md#dae-13)。
- 说明：对 daemon 子进程发送 SIGKILL，等待 daemon PID 改变、能力标记恢复，检查
  supervisor PID 保持不变以及正确的父子关系。
- 缺口：此 VM 场景只直接验证一次崩溃拉起；完整退避序列与稳定运行重置由单元和进程
  fixture 验证。

## SVI-03

**主动停止服务后不被自动拉起，手动启动恢复运行**

- 测试层：QEMU/KVM + 真实 SysVinit init 脚本
- 状态：`已覆盖`
- 证据：[guest-tests.sh initial](../../../../scripts/qemu/sysvinit/guest-tests.sh)。
- 说明：调用 `/etc/init.d/lkit.service stop`，按实际进程参数验证 daemon 与 supervisor
  均消失，此后每秒检查一次，持续 12 秒；观察窗口超过 10 秒退避上限。手动 start 后
  再次验证父子进程及能力标记。

## SVI-04

**启用的 daemon 在真实重启后自动启动**

- 测试层：QEMU/KVM + SysVinit 真实重启
- 状态：`已覆盖`
- 证据：[guest-tests.sh enabled-reboot](../../../../scripts/qemu/sysvinit/guest-tests.sh)、
  [宿主重启与 SSH 等待](../../../../scripts/test-qemu-sysvinit.sh)。
- 说明：宿主和 guest 都核对 boot ID 改变；guest 再次核实 SysVinit 身份，S 启动链接
  存在，daemon 无须测试手动 start 即恢复运行并发布能力标记。

## SVI-05

**禁用的 daemon 在真实重启后保持停止，手动启动不改变禁用状态**

- 测试层：QEMU/KVM + SysVinit 真实重启
- 状态：`已覆盖`
- 证据：[guest-tests.sh enabled-reboot / disabled-reboot](../../../../scripts/qemu/sysvinit/guest-tests.sh)。
- 说明：`update-rc.d lkit.service disable` 清除 S 链接并保留停止链接，随后停服务并
  重启。boot ID 改变后确认无 S 链接，连续 12 秒逐秒检查 daemon 与 supervisor 均不存在。
  手动 start 恢复运行和能力标记，S 链接仍不存在。

## SVI-06

**self remove 停止 daemon 并幂等清理服务和全部启动链接**

- 测试层：QEMU/KVM + 真实 SysVinit
- 状态：`已覆盖`
- 证据：[guest-tests.sh disabled-reboot](../../../../scripts/qemu/sysvinit/guest-tests.sh)。
- 说明：对已禁用但手动运行的服务执行 `self remove`；daemon 与 supervisor 消失，
  服务原件、init.d 链接和全部 runlevel 链接删除，CLI 二进制保留；再次移除成功且没有
  遗留进程。
