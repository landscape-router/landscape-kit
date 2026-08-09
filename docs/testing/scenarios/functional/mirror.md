# 主机换源场景

`lkit set-mirror` 切换当前主机 Linux 软件包管理器软件源：检测发行版、按家族重写
apt/dnf/pacman 源文件，换源前自动备份原文件，可一键恢复。

## MIR-01

**识别发行版与软件包管理器并列出可用镜像**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::detect` 测试](../../../../crates/lkit-cli/src/mirror/detect.rs)、[`set-mirror --list`](../../../commands/mirror.md)
- 说明：覆盖 Debian/Ubuntu（含 VERSION 行解析代号）、CentOS 7 与 Stream、Fedora、
  Rocky、AlmaLinux、Arch；不支持发行版与不可读 os-release 报错。

## MIR-02

**apt 换源重写保持协议并保留未识别 URL**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::apt` 测试](../../../../crates/lkit-cli/src/mirror/apt.rs)
- 说明：覆盖 one-line 与 deb822 格式、Debian security/backports/ports 路径、
  Ubuntu `-ports` 先于 `ubuntu` 的替换顺序、镜像主机不重复替换、主机名子串不误替换、
  已识别镜像（TUNA/阿里云/USTC）之间互转且自定义主机不受影响、Debian security
  默认保留官方（`--replace-security` 才替换）、Ubuntu security 始终随主仓库替换。
  已处于目标镜像/官方状态时 `apply` 为成功 no-op（不保留备份），没有任何可识别
  URL 且未处于目标时报错。

## MIR-03

**dnf/yum 按仓库块转换并跳过无 baseurl 的块**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::dnf` 测试](../../../../crates/lkit-cli/src/mirror/dnf.rs)
- 说明：覆盖 `# baseurl=` 解注释与重写、mirrorlist/metalink 注释、Fedora/EPEL、
  CentOS 7/Stream、Rocky、Alma 主机映射、官方反向恢复、自定义主机不动、
  已识别镜像之间互转；已处于目标镜像/官方状态时 `apply` 为成功 no-op（不保留备份），
  没有任何可识别 URL 且未处于目标时报错。

## MIR-04

**pacman 生成单 Server 的新 mirrorlist**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::pacman` 测试](../../../../crates/lkit-cli/src/mirror/pacman.rs)
- 说明：四个镜像各生成恰好一个 `Server =` 行，模板含 `$repo/os/$arch`。

## MIR-05

**换源前自动备份并可恢复原源**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::apt::restore_files` 测试](../../../../crates/lkit-cli/src/mirror/apt.rs)、
  [`console::tests::mirror`](../../../../crates/lkit-cli/src/console/tests/mirror.rs)
  的 apply/restore 端到端测试
- 说明：备份保留相对路径，恢复写回目标根目录后删除备份；无备份时恢复报错。控制台
  确认层端到端测试（`test-support` 特性下）注入临时根路径并允许非 root，验证真实
  的备份、重写与恢复流程不触碰本机源。
- 缺口：真实 `/var/lib/lkit/mirror-backup` 下对主机源文件的备份/恢复端到端验证
  （需要 root 或容器）。

## MIR-06

**控制台换源面板提供镜像选择与恢复**

- 测试层：Rust 控制台测试
- 状态：`已覆盖`
- 证据：[`console::tests::mirror`](../../../../crates/lkit-cli/src/console/tests/mirror.rs)
- 说明：菜单导航、面板渲染、行选择边界、确认层打开/关闭、确认执行、Debian 确认层
  security 开关（默认不勾选、Space 切换、非 Debian 隐藏）与鼠标点击命中；确认执行
  （`test-support`）在注入的临时路径下断言源文件实际改写为镜像、备份内容为原文件、
  恢复后原内容写回且备份删除。

## MIR-07

**换源要求 root 且非交互无参数时报用法错误**

- 测试层：CLI
- 状态：`已覆盖`
- 证据：[`set-mirror` 命令](../../../commands/mirror.md)
- 说明：非 root 执行换源/恢复报错阻断；`--non-interactive` 无参数报参数使用错误；
  `--list`/`--show` 只读不需要 root。
- 缺口：自动化脚本断言缺少直接断言（手工冒烟）。
