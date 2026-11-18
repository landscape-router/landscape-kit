# Windows 客户端的二层验收

在 **Test flare e2e** 手动选择 `windows-npcap`。Windows 托管 runner 编译当前提交的 `lflare.exe`。Linux runner 在 containerlab 中启动两个同名路由器，再把 Windows QEMU 虚拟机的唯一网卡通过 TAP 接入同一座桥。Windows 使用 Npcap 发现设备，按两个控制台指纹分别认证，读取映射后端返回的 A、B。两个响应均匹配且本轮验收结束才会通过。[工作流](../../.github/workflows/test-flare.yml) · [客户端断言](../../scripts/flare/windows-npcap-smoke.ps1)

## 准备私有测试镜像

使用适用于该 CI 环境的 Windows x64 镜像，保存为单个可独立启动的 qcow2。虚拟机使用传统 BIOS、IDE 系统盘和光驱、Intel e1000 网卡和 COM1。在镜像中安装 PowerShell 7、Npcap 及 e1000 驱动。基础镜像保存在自己的私有存储中，每次运行的系统盘写入临时快照。[QEMU 网络设备](https://www.qemu.org/docs/master/system/devices/net.html) · [Npcap 安装说明](https://npcap.com/guide/npcap-users-guide.html)

在测试镜像中以管理员身份把仓库的 `scripts/flare/windows-qemu-guest.ps1` 复制到 `C:\FlareLab\guest.ps1`，注册启动任务后关闭虚拟机并保存镜像。该任务从标签为 `FLARETEST` 的只读光盘读取本次客户端、两个预期指纹与临时夹具密码，查找测试网卡，再从 COM1 返回断言结果。镜像无需管理网卡或远程登录账户。

```powershell
$action = New-ScheduledTaskAction -Execute (Get-Command pwsh).Source `
    -Argument '-NoProfile -NonInteractive -File C:\FlareLab\guest.ps1'
Register-ScheduledTask -TaskName FlareNpcapSmoke -Action $action `
    -Trigger (New-ScheduledTaskTrigger -AtStartup) -User SYSTEM -RunLevel Highest
```

在仓库 Actions secret `FLARE_WINDOWS_IMAGE_URL` 中设置该镜像的私有 HTTPS 下载地址，在 Actions variable `FLARE_WINDOWS_IMAGE_SHA256` 中设置完整文件的 SHA-256。手动选择 Windows 场景时，两项均为必需，镜像校验失败会中止作业。常规 push 上的 Windows 作业只编译客户端、解析验收脚本并报告 runner 的驱动状态。使用已有 Windows 测试机时可以直接运行 [FLR-42 的命令](../testing/scenarios/functional/flare.md#flr-42)。
