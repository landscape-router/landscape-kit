# Landscape Terrain（flare）文档

flare 是 Landscape 路由器的 L2 防失联通道：主机通过以太网帧与路由器建立加密
TCP-over-IP 隧道（`lflare` 客户端 ↔ `lkit flare` 服务端），用于常规网络路径
不可用时的应急管理连接。

- [协议规范](protocol.md)：帧格式、密钥计划、握手流程、隧道与端口转发、服务端防护
- [测试体系](testing.md)：Docker L2 bridge 双容器 e2e 的入口、拓扑与日志契约
- [测试场景](scenarios.md)：`FLR-01` 至 `FLR-18` 场景目录
