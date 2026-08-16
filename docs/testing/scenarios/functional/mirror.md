# 主机换源场景

`lkit set-mirror` 切换当前主机 Linux 软件包管理器软件源：检测发行版、按家族重写
apt/dnf/pacman 源文件，换源前自动备份原文件，可一键恢复。

## MIR-01

**识别发行版与软件包管理器并列出可用镜像**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::detect` 测试](../../../../crates/lkit-cli/src/mirror/detect.rs)、[`set-mirror --list`](../../../commands/mirror.md)
- 说明：覆盖 Debian/Ubuntu（含 VERSION 行解析代号）、Fedora、Rocky、AlmaLinux、
  Arch；不支持发行版与不可读 os-release 报错。

## MIR-02

**apt 换源按条目解析重写并保留未识别 URL**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::apt::parse` 测试](../../../../crates/lkit-cli/src/mirror/apt/parse.rs)
- 说明：先按 one-line/deb822 格式解析为条目（类型、URI、suites、components、
  是否注释、CD 源），再对命中的条目做 URI 片段级替换。覆盖 one-line 与 deb822
  格式（含 `[options]`、禁用行、多 URI 的 `URIs:` 行、注释）、Debian
  security/backports/ports 路径、Ubuntu `-ports` 先于 `ubuntu` 的替换顺序、镜像主机
  不重复替换、主机名子串不误替换、已识别镜像（USTC/腾讯云/华为云/阿里云/NJU/SJTU/
  ZJU/LZU/HUST/BFSU/TUNA 十一个）之间互转且自定义
  主机不受影响、Debian security 默认保留官方（`--replace-security` 才替换）、Ubuntu
  security 始终随主仓库替换；带显式端口或凭证的 URL 按归一化后的主机匹配重写（端口
  丢弃、凭证保留，IPv6 字面量除外）；不符合规范的一行多 URL（第二个 URL 落在
  components 位置）同样识别为 URI 一并重写，其余字节原样保留；无可识别 URL 时条目
  与文件字节级原样保留（`rewrite` 返回 `None` 不写文件）；已处于目标镜像/官方状态
  时 `apply` 为成功 no-op（不保留备份、不触碰已有备份）。

## MIR-08

**仅 CD 源或仅自定义源时换源自动兜底（转换 CD 行或合成新条目）**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::apt::parse` 测试](../../../../crates/lkit-cli/src/mirror/apt/parse.rs)
  与 [`mirror::apt` apply 测试](../../../../crates/lkit-cli/src/mirror/apt/mod.rs)
- 说明：没有任何条目可重写且未处于目标状态时，`apply` 不再报错：存在启用的
  `deb cdrom:` 条目时把该行转换为所选镜像（保留 suites/components，被注释的
  cdrom 行不转换）；否则用检测到的代号合成新条目追加到 `sources.list`（不存在时
  创建 `sources.list.d/lkit-mirror.list` 并在备份目录放空占位，`--restore` 后不
  残留）。`sources.list` 为空文件、只有注释、或系统里完全没有源文件（含
  `sources.list.d` 目录缺失，自动创建）时同样合成。Ubuntu 按运行时架构（`uname -m`）
  选择仓库：arm64/armhf/riscv64/ppc64el/s390x 转 `/ubuntu-ports`（官方回落
  `ports.ubuntu.com`），其余架构转 `/ubuntu`。`test-support` 下验证
  CD-only、custom-only、空文件、纯注释、无任何源文件五种场景的换源成功、备份与
  恢复语义（恢复后原内容写回、备份删除）；重复执行同一目标为 no-op 且保留上一轮
  备份（`--restore` 仍可取回最原始源）。Debian security 合成行默认官方、
  `--replace-security` 时随镜像；`official` 目标合成官方主机条目。

## MIR-03

**dnf/yum 按仓库块转换并跳过无 baseurl 的块**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::dnf::parse` 测试](../../../../crates/lkit-cli/src/mirror/dnf/parse.rs)
- 说明：覆盖 `# baseurl=` 解注释与重写、mirrorlist/metalink 注释、Fedora/EPEL、
  Rocky、Alma 主机映射、官方反向恢复、自定义主机不动、
  已识别镜像之间互转；已处于目标镜像/官方状态时 `apply` 为成功 no-op（不保留备份），
  没有任何可识别 URL 且未处于目标时报错。

## MIR-04

**pacman 生成单 Server 的新 mirrorlist**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::pacman` 测试](../../../../crates/lkit-cli/src/mirror/pacman/mod.rs)
- 说明：十二个镜像各生成恰好一个 `Server =` 行，模板含 `$repo/os/$arch`。

## MIR-05

**换源前自动备份并可恢复原源**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[`mirror::common::restore_files` 测试](../../../../crates/lkit-cli/src/mirror/common.rs)、
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

## MIR-09

**真实发行版容器内的换源 E2E（Debian/Ubuntu/Fedora/Arch）**

- 测试层：Docker E2E
- 状态：`已覆盖`
- 证据：[`scripts/test-docker-mirrors.sh`](../../../../scripts/test-docker-mirrors.sh)、
  [`run-distro.sh`](../../../../scripts/docker-mirrors/run-distro.sh)、
  [测试说明](../../../testing/docker-mirrors.md)
- 说明：用 rust:bookworm 构建生产二进制，经 docker 命名卷挂载进
  `debian:bookworm`/`ubuntu:24.04`/`fedora:latest`/`archlinux:latest` 容器（root，无需
  test-support），依次验证 tuna 切换、`--restore` 恢复（内容逐字节还原、备份删除）、
  aliyun 互转、official 恢复。Debian 覆盖 one-line 与 deb822 两种镜像布局，并验证
  "仅 CD 源"兜底（cdrom 行转换为镜像、保留 suites/components）、空 sources.list
  合成条目、`--check` 格式检查（问题文件退出码非 0 且指出行号、干净文件退出 0）；
  Ubuntu 覆盖 deb822 布局、security 并入主仓库路径，且断言按容器架构选择
  `archive.ubuntu.com`/`/ubuntu` 或（CI 的 aarch64 runner 上）
  `ports.ubuntu.com`/`/ubuntu-ports`（含 CD 源兜底）；
  Fedora 覆盖 `#baseurl=` 解注释、metalink 注释与 fedora/epel 映射（占位主机先 sed
  为规范官方主机）；Arch 验证 mirrorlist 整体重新生成（恰好一个 Server）。
- 缺口：Rocky、AlmaLinux 尚未纳入容器矩阵（机制相同，
  可在 compose 列表扩充）。

## MIR-10

**软件源格式检查与无法识别行报告**

- 测试层：Rust 单元 + CLI + Docker E2E
- 状态：`已覆盖`
- 证据：[`mirror::apt::parse` 诊断测试](../../../../crates/lkit-cli/src/mirror/apt/parse.rs)、
  [`mirror::apt` check_format 测试](../../../../crates/lkit-cli/src/mirror/apt/mod.rs)、
  [`set-mirror --check`](../../../commands/mirror.md)
- 说明：解析器按行号报告 `NotADebLine`/`MissingUri`/`NotAField`/`StanzaWithoutUris`
  四类异常（one-line 混入 deb822、括号不配对、缺 URI、deb822 混入 one-line、stanza
  缺 URIs），纯注释行不算异常；多行 deb822 stanza 内的异常精确到出错行（stanza 级
  的缺 URIs 报告在首字段行）；没有任何源文件时 `--check` 视为干净；`--check` 只读
  检查并逐行输出、有问题是退出码 `1`；`apply` 前同样先检查，无法识别的行字节级
  保留并在结果中提示数量。

## MIR-11

**镜像可用性探测：不可用不可选、未知确认时警告**

- 测试层：Rust 单元 + 控制台测试
- 状态：`已覆盖`
- 证据：[`mirror::availability` 测试](../../../../crates/lkit-cli/src/mirror/availability.rs)、
  [`console::tests::mirror`](../../../../crates/lkit-cli/src/console/tests/mirror.rs)
- 说明：换源前并行 HEAD 探测每个镜像站上"当前发行版"的真实文件（Debian/Ubuntu
  `dists/<代号>/Release`、Fedora 换源后实际写入的
  `fedora/linux/releases/<主版本>/Everything/<架构>/os/repodata/repomd.xml`、
  Rocky/AlmaLinux `repomd.xml`、Arch `core.db`；Ubuntu 按运行时架构选
  `ubuntu-ports`；dnf 家族按 `VERSION_ID` 主版本构造路径）。纯函数 URL 构造单测
  覆盖各家族与缺失输入（无代号/无 VERSION_ID → 未知）；`Official` 恒可用不探测。
  明确 404 → 不可用：控制台面板置灰且导航/确认跳过、显式 `set-mirror` 直接拒绝；
  网络失败/超时/TLS/403 → 未知：仍可选，确认层显示警告行；探测结果未就绪时全部
  视为可用。`--list` 标注 `[可用]/[不可用]/[未知]`。
- 缺口：真实网络探测（404/403/超时分支）依赖镜像站实况，未纳入自动化断言；
  面板 worker 线程（探测中提示行）由状态注入测试覆盖，不发起真实请求。
