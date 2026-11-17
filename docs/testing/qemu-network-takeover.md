# QEMU/KVM 网络接管测试

`scripts/test-qemu-network-takeover.sh` 在硬件虚拟化的 x86_64 Linux 主机上构建一个
Debian systemd 根文件系统，并运行两台从同一基线派生的干净虚拟机。虚拟机包含真实
NetworkManager、firewalld、systemd-resolved、OpenSSH 与两个 virtio 网卡；Landscape
由生产 `lkit` 从正式 provider 下载，不使用 fake systemctl 或 fake `ip`。

第一台虚拟机完成网络接管后从 `192.168.10.1` 建立 SSH，随后在未确认时重启。测试等待
boot rollback 恢复 WAN SSH 和三个宿主服务，并断言事务为 `rolled_back`、未提交的安装状态
和整个 `data/` 已删除。第二台虚拟机再次接管网络，从新 LAN 地址运行 `lkit network
confirm`，断言事务和安装状态提交且恢复 unit 已清理。测试同时验证确认前保留 WAN
地址，确认后按事务中的 Static 或 DHCP 计划验证 WAN IPv4 配置。

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
本脚本不直接验证确认超时、提交后重启、断电式终止或 OpenRC／SysVinit 网络接管。
Alpine QEMU 覆盖 OpenRC 的 daemon 拉起、崩溃恢复和启动顺序，不替代网络接管验收。

NetworkManager 的重启恢复还需要控制 IPv6 RA 时序的验收：daemon 提前拉起 WAN 后，
内核自动配置的 IPv6 可能使 NetworkManager 采用 external connection，原 DHCP 连接
没有激活。此时服务 active 和事务 rolled_back 不能证明 WAN IPv4 恢复；
缺口与断言见 [NET-15](scenarios/functional/network-takeover.md#net-15)。

本地实测应在专用测试主机或独立网络命名空间内运行：脚本会创建 TAP 并配置
`192.168.10.2/24`。使用容器时，仓库和预编译二进制只读挂载，日志目录单独可写；
不共享宿主网络、PID 命名空间或 Docker socket。通过 `LKIT_QEMU_ARTIFACT_DIR`
指定持久日志目录，默认临时目录会在退出时删除。
