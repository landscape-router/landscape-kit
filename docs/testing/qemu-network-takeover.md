# QEMU/KVM 网络接管测试

`scripts/test-qemu-network-takeover.sh` 在硬件虚拟化的 x86_64 Linux 主机上构建一个
Debian systemd 根文件系统，并运行两台从同一基线派生的干净虚拟机。虚拟机包含真实
NetworkManager、firewalld、systemd-resolved、OpenSSH 与两个 virtio 网卡；Landscape
由生产 `lkit` 从正式 provider 下载，不使用 fake systemctl 或 fake `ip`。

第一台虚拟机完成网络接管后从 `192.168.10.1` 建立 SSH，随后在未确认时重启。测试等待
boot rollback 恢复 WAN SSH 和三个宿主服务，并断言事务为 `rolled_back`。第二台虚拟机
再次接管网络，从新 LAN 地址运行 `lkit network confirm`，断言事务和安装状态提交且恢复
unit 已清理。

测试要求 `/dev/kvm` 可读写并明确拒绝 TCG fallback。本地执行：

```sh
cargo build --locked --release -p lkit-cli --bin lkit
sudo env LKIT_QEMU_PREBUILT="$PWD/target/release/lkit" \
  scripts/test-qemu-network-takeover.sh
```

GitHub workflow 在相关 PR、main push、每周计划和手工触发时自动运行，只授予
`contents: read` 与 `actions: read`，不使用 repository secrets。该 check 初期为
observational，不应加入 branch protection required checks。workflow 使用只读 Actions API
报告最近 20 次完成结果；只有连续 20 次成功后才应由仓库管理员把它提升为 required check。
