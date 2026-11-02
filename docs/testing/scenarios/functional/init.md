# 初始化系统与 Cgroup 准备

## INIT-01

**按实际运行方式并列识别 systemd、OpenRC、SysVinit，只生成当前后端的检查**

- 测试层：Rust 单元（纯输入证据与假查询）
- 状态：`已覆盖`
- 证据：[检查规格](../../../check.md)、[识别测试](../../../../lkit-cli/src/service/manager.rs)、[检查分发测试](../../../../lkit-cli/src/check/init.rs)
- 说明：兼容工具和 init.d 目录不能单独决定后端；systemd 优先于残留 OpenRC 标记，OpenRC 优先于 SysV 兼容工具。未知环境明确报告 unknown，不默认声称为 systemd。其他后端不查询 OpenRC boot 注册。

## INIT-02

**OpenRC 准备必须完成当前挂载和 boot 注册，拒绝不安全转换和虚假成功**

- 测试层：Rust 单元（假系统操作、临时目录）
- 状态：`已覆盖`
- 证据：[准备测试](../../../../lkit-cli/src/deployment/cgroup.rs)、[OpenRC 检查](../../../../lkit-cli/src/check/openrc.rs)
- 说明：已有挂载仍须注册 boot；命令失败、CPU controller 缺失、注册未生效、只读／v1／混合／冲突挂载均不能报告准备成功。boot 链接必须指向可执行的 cgroups 原件。

## INIT-03

**控制台二次确认立即准备和开机启用，失败或断开时保留安装门禁**

- 测试层：Ratatui TestBackend、假 worker 通道
- 状态：`已覆盖`
- 证据：[控制台规格](../../../interaction/console.md)、[交互测试](../../../../lkit-cli/src/console/tests/cgroup.rs)
- 说明：Esc 取消、两个强制命令可见、不支持的主机不提供修复、成功刷新并关闭旧阻断框、其他错误继续阻断。测试不执行真实服务命令。

## INIT-04

**真实 OpenRC 开机时 Cgroup 先于 Landscape 就绪，跨重启保持启用**

- 测试层：OpenRC 虚拟机兼容性
- 状态：`部分覆盖`
- 证据：[OpenRC 定义测试](../../../../lkit-cli/src/service/openrc.rs)、[Alpine 虚拟机测试入口](../../../../scripts/qemu/alpine/guest-tests.sh)、[启动顺序 fixture](../../../../scripts/qemu/alpine/router-fixture.c)
- 说明：虚拟机断言 boot 注册缺失时检查阻断、恢复注册和真实重启后通过；Landscape fixture 要求 daemon 与 cgroups 已完成启动、CPU controller 可读。定义测试证明 daemon 不声明 cgroups 硬依赖。
- 缺口：虚拟机通过系统命令设置注册；控制台确认到准备 worker 再到真实重启的完整链路尚未直接覆盖。脚本包含断言不等于本次 CI 已执行通过。
