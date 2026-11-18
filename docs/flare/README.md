# Landscape Terrain（Flare）救援通道

在管理 IP 不可达时，将救援电脑直接接到路由器所在的二层网络，运行 `lflare`。选择网卡并发现设备，再从列表选择目标。设备可以同名。每次登录前核对完整指纹，使用 Landscape 管理员密码或本机 Linux root 密码登录，随后在会话页添加端口映射。

脚本可先列出设备，再用本次确认的完整指纹建立映射。省略 `--password-file` 时在终端隐藏输入密码。

```sh
lflare discover --dev eth0 --json
lflare cli --dev eth0 --target "$FINGERPRINT" --auth landscape --user admin --forward 8443:6443
lflare cli --dev eth0 --target "$FINGERPRINT" --auth root --forward 2222:22
```

通过第三条命令访问 SSH 时，另开终端运行 `ssh -p 2222 root@127.0.0.1` 并完成 SSH 自己的认证。Flare 客户端不保存设备身份或登录密码。设备本机可运行 `lkit flare setup` 核对指纹。私有发现需要在扫描和连接时提供同一个发现 PSK。连接旧版 daemon 可在 TUI 按 F5，或使用 `lflare cli --legacy --psk "$OLD_CODE" --dev eth0 --forward 8443:6443`。

- [协议规范](protocol.md)
- [CI 测试与拓扑](testing.md)
- [链路测试目录](scenarios.md)
- [设备选择与认证验收场景](../testing/scenarios/functional/flare.md#flr-37)
