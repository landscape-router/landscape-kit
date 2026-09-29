# Landscape Terrain（flare）文档

flare 是 Landscape 路由器的 L2 防失联通道：主机通过以太网帧与路由器建立加密
TCP-over-IP 隧道（`lflare` 客户端 ↔ `lkit flare` 服务端），用于常规网络路径
不可用时的应急管理连接。

- [协议规范](protocol.md)：帧格式、密钥计划、握手流程、隧道与端口转发、服务端防护
- [测试体系](testing.md)：Docker L2 bridge 双容器 e2e 的入口、拓扑与日志契约
- [测试场景](scenarios.md)：`FLR-01` 至 `FLR-29` 场景目录

## 客户端用法

`lflare`（Windows 双击或终端直接运行）默认进入交互 TUI：表单输入 psk、设备、
token 等，最后聚焦「连接」按钮并按 Enter 握手；连接成功后在会话页临时添加或删除
端口映射。脚本环境继续使用 `lflare cli --psk … --dev eth0 --forward 2222:6443`。

退出交互与 console 的约定一致：会话页按 `q`/`Esc` 先弹出「断开连接」确认层
（Enter 确认、Esc 取消），`Ctrl-C` 任何时候立即退出（含映射编辑器与确认层）；
表单页退出用 `Ctrl-Q`/`Ctrl-C`，`Esc` 只用于收起设备选择器（取消时回滚到
打开前的选择）。编辑字段会自动清除上一轮的校验错误。

TUI 需要至少 60x24 的终端：小于该尺寸时表单与会话页都渲染整屏的
「终端过小」提示（放大窗口即恢复），而不是挤压字段；表单中展开的设备列表会
按剩余高度收缩可见行数，溢出提示行始终保留。
