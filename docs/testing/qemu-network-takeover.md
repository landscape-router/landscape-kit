# QEMU/KVM 网络接管测试

`scripts/test-qemu-network-takeover.sh` 在硬件虚拟化的 x86_64 Linux 主机上构建一个
Debian systemd 根文件系统，并运行三台从同一基线派生的干净虚拟机。虚拟机包含真实
NetworkManager、firewalld、systemd-resolved、OpenSSH 与两个 virtio 网卡；Landscape
由生产 `lkit` 从正式 provider 下载，不使用 fake systemctl 或 fake `ip`。
每台 guest 的 `<scenario>-environment.log` 保存发行版、内核、PID 1 可执行文件和 lkit 版本，
并断言 PID 1 为 systemd、lkit 检测到 systemd 后端。

第一台虚拟机完成网络接管后从 `192.168.10.1` 建立 SSH，随后在未确认时重启。测试等待
boot rollback 恢复 WAN SSH 和三个宿主服务，并断言事务为 `rolled_back`、未提交的安装状态
和整个 `data/` 已删除。第二台虚拟机再次接管网络，从新 LAN 地址运行 `lkit network
confirm`，断言事务和安装状态提交且恢复 unit 已清理。测试同时验证确认前保留 WAN
地址，确认后按事务中的 Static 或 DHCP 计划验证 WAN IPv4 配置。

第二台虚拟机同时执行正式业务备份与离线恢复：按 ID 和外部文件恢复并确认，随后
逐一验证主动回滚、真实 600 秒超时、未确认整机重启及强杀 QEMU 后冷启动的恢复结果。
启动恢复时复用原两块磁盘，guest CPU 运行前通过 QMP 保持 WAN 断开；所有场景都从
独立 LAN 客户端检查登录、前端、DNS 与导出的 DHCP／网络配置。最终再次确认恢复并
重启，随后移除 daemon 再重启，确认已提交业务仍可自动启动。

第三台虚拟机由真实公开的 `v0.7.1-rc2` 安装，停服迁至独立安装盘后通过 `self upgrade` 到公开的
`v0.7.1-rc4`；未发布候选以明确区分的原子替换步骤部署。候选修复旧服务定义时保持运行状态，
再验证整机重启及业务配置。固定版本、详细断言与剩余边界见
[真实安装与恢复场景 JNY-01 至 JNY-05](scenarios/functional/real-install-recovery.md)。

测试要求 `/dev/kvm` 可读写并明确拒绝 TCG fallback。本地执行：

```sh
cargo build --locked --release -p lkit-cli --bin lkit
sudo env LKIT_QEMU_PREBUILT="$PWD/target/release/lkit" \
  scripts/test-qemu-network-takeover.sh
```

GitHub workflow 路径过滤包含 CLI、内部 crates、Terrain 协议库与 Cargo 配置，在相关 PR、main push、每周计划和手工触发时自动运行，只授予
`contents: read` 与 `actions: read`，不使用 repository secrets。该 check 初期为
observational，不应加入 branch protection required checks。workflow 使用只读 Actions API
报告最近 20 次完成结果；只有连续 20 次成功后才应由仓库管理员把它提升为 required check。

## 验证边界

该脚本使用 NetworkManager，systemd-networkd 在基线中禁用；networkd 的真实 manager
保存和恢复由 [nspawn smoke](nspawn-systemd.md) 验证，但其接口发现和 Landscape 为 fixture。
因此 networkd 管理真实网卡时的接管、SSH 重连与重启恢复仍缺少 QEMU 验收。
本脚本的实际超时和突然终止验证针对 restore 待确认阶段；首次安装接管的实际超时、
所有持久化写入边界，以及 OpenRC／SysVinit 完整网络接管仍需对应验收。
强杀 QEMU 模拟 guest 丢失内核内存，宿主缓存及物理磁盘仍正常，不等同于存储设备断电。
Alpine QEMU 覆盖 OpenRC 的 daemon 拉起、崩溃恢复和启动顺序，Debian/SysVinit QEMU
覆盖 daemon 生命周期与启用/禁用后的重启，均不替代网络接管验收。
完整范围见 [QEMU 服务管理后端验收](qemu-service-managers.md)。

NetworkManager 重启恢复使用确定性的故障注入：在 daemon 拉起 WAN 后等待 IPv6 RA，
再启动 NetworkManager；原 WAN profile 暂时移走，断言事务保持 `rolling_back` 且待提交
状态未提前清理。放回 profile 后只 reload 配置，由 daemon 自动激活原 UUID，恢复 IPv4、
默认网关与 WAN SSH。绝对和相对自定义启动链接保持原样。对应 [NET-15/16](scenarios/functional/network-takeover.md#net-15)。

本地实测应在专用测试主机或独立网络命名空间内运行：脚本会创建 TAP 并配置
`192.168.10.2/24`。使用容器时，仓库和预编译二进制只读挂载，日志目录单独可写；
不共享宿主网络、PID 命名空间或 Docker socket。通过 `LKIT_QEMU_ARTIFACT_DIR`
指定持久日志目录，默认临时目录会在退出时删除。
