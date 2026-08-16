# `lkit set-mirror`

切换当前主机的 Linux 软件包管理器软件源（换源）。支持 apt（Debian/Ubuntu）、dnf/yum
（Fedora、CentOS 7、CentOS Stream、Rocky、AlmaLinux）与 pacman（Arch Linux），预置
清华 TUNA、阿里云、中科大 USTC 与六所大学镜像（南大 NJU、上交 SJTU、浙大 ZJU、
兰大 LZU、北外 BFSU、华科 HUST）和官方源恢复。

```text
lkit set-mirror <MIRROR> [--yes] [--replace-security]
lkit set-mirror --list
lkit set-mirror --show
lkit set-mirror --check
lkit set-mirror --restore [--yes]
```

`MIRROR` 为 `tuna`、`aliyun`、`ustc`、`nju`、`sjtu`、`zju`、`lzu`、`bfsu`、`hust` 或
`official`。无参数且连接终端时进入交互选择：
先检测发行版，再列出可用镜像供选择，确认后执行。`--non-interactive` 且无参数时报参数
使用错误（退出码 `2`）。

## 格式检查

`--check` 只读检查受管软件源文件的格式（仅 apt 家族；dnf/pacman 主机提示不支持），
不需要 root。按 one-line/deb822 解析全部受管文件，报告无法识别的行与行号：

- `NotADebLine`：one-line 文件里类型不是 `deb`/`deb-src` 的内容行（可能是混入的
  deb822 段落或其他内容）；
- `MissingUri`：`deb`/`deb-src` 行缺少 URI（缺失，或 `[options]` 括号不配对）；
- `NotAField`：deb822 文件里不是 `字段: 值` 的行（可能是混入的 one-line 行）；
- `StanzaWithoutUris`：deb822 stanza 有字段但缺 `URIs`。

存在问题时逐行打印 `文件: 第 N 行：描述` 并返回退出码 `1`；干净文件打印
"no source format issues found" 并返回 `0`。没有任何源文件时视为干净（与 `apply`
的兜底语义一致）。

换源（`apply`）前也会先做同样的格式检查：能识别的条目照常重写，无法识别的行
字节级原样保留，并在结果里提示 "另有 N 行无法识别，已保留原样"。

## 检测与支持范围

从 `/etc/os-release` 的 `ID` 读取发行版家族：

| 家族 | 软件包管理器 | 修改的文件 |
| --- | --- | --- |
| Debian | apt | `/etc/apt/sources.list`、`/etc/apt/sources.list.d/*`（含 deb822 `.sources`；兜底时可能新建 `sources.list.d/lkit-mirror.list`） |
| Ubuntu | apt | 同上 |
| Fedora、CentOS 7、CentOS Stream、Rocky、AlmaLinux | dnf/yum | `/etc/yum.repos.d/*.repo` |
| Arch | pacman | `/etc/pacman.d/mirrorlist` |

不支持的其他发行版（如 Alpine）直接报错，不修改任何文件。检测失败同样报错阻断。

## 重写规则

apt 换源先按源文件格式（one-line `sources.list` 与 deb822 `*.sources`）把内容解析为
条目（类型、URI、suites、components、options、是否被注释、是否 CD 源），再对命中的
条目做 URI 替换：只替换 URI 片段，其余字节（注释、缩进、其他字段）逐字保留：

- Debian：`deb.debian.org/debian`、`deb.debian.org/debian-security`、
  `deb.debian.org/debian-backports`、`deb.debian.org/debian-ports` 与
  `security.debian.org/debian-security` 的 `http(s)://` 主机替换为镜像主机；
- Ubuntu：`archive.ubuntu.com/ubuntu`、`security.ubuntu.com/ubuntu`、
  `archive.ubuntu.com/ubuntu-ports` 与 `ports.ubuntu.com/ubuntu-ports` 替换为镜像主机
  （`-ports` 映射到镜像的 `ubuntu-ports` 路径）；
- 原始协议（`http://` 或 `https://`）保持不变，未被识别的 URL（如 PPA 与自定义源）
  原样保留；带显式端口（`deb.debian.org:443`）或凭证（`user:pass@`）的 URL 同样
  按主机匹配并重写，端口随主机一起丢弃、凭证保留；
- 不符合规范的行也尽力修复：一行里多出的 URL（重复粘贴、或误放在 components 位置）
  同样识别为 URI 一并重写，避免换源后"一半镜像一半官方"；缺 suites/components、
  括号不配对等无法安全猜测的行保持原样；
- 除官方主机外，已识别的九个公共镜像（TUNA、阿里云、USTC、NJU、SJTU、ZJU、LZU、
  BFSU、HUST）之间也可以互转：选择其中一个时，其余镜像的 URL 会一并替换为所选
  镜像；自定义内网镜像等未识别主机不受影响；
- Debian 的独立 security 仓库（`deb.debian.org/debian-security` 与
  `security.debian.org/debian-security`）默认**不替换**，保持官方源（安全补丁讲究
  时效，部分镜像站也不镜像 security）；需要一并替换时加 `--replace-security`。
  Ubuntu 的 security 内容与主仓库合并镜像，没有独立路径，始终随主仓库一起替换，
  该选项对 Ubuntu 不生效。

没有任何条目可重写（例如离线安装后只有 `deb cdrom:` 行、或默认源被换成自定义/内网
源）且未处于目标状态时，不再报错，而是自动兜底。以下情况都会进入兜底：

- 存在启用的 `deb cdrom:` 条目 → 把该条目转换为所选镜像（保留其 suites/components，
  如 `deb cdrom:[...]/ bookworm contrib main` → `deb https://mirrors.tuna.../debian
  bookworm contrib main`）；被注释的 cdrom 行不转换；Ubuntu 在 arm64 等 ports 架构
  （`uname -m` 判定）上转 `/ubuntu-ports`（官方回落 `ports.ubuntu.com`），其余架构
  转 `/ubuntu`，避免换源后 apt 找不到包；
- 否则用检测到的发行版代号合成新条目追加到 `sources.list`（不存在时创建
  `sources.list.d/lkit-mirror.list`）：Debian 追加 `main contrib non-free` 主仓库与
  官方 security 行（`--replace-security` 时 security 一并换到镜像），Ubuntu 追加
  `main universe restricted multiverse` 主仓库与 security 行（ports 架构同样落
  `/ubuntu-ports`）；`official` 目标同样合成官方主机条目；
- `sources.list` 为空文件、只有注释、或系统里完全没有源文件（`sources.list` 与
  `sources.list.d` 都不存在，`sources.list.d` 目录缺失也会自动创建）时同样合成
  新条目，`--restore` 后文件回到原状态（空文件/不存在）；
- 只有检测不到代号（Debian/Ubuntu 的 os-release 缺 `VERSION`/`VERSION_CODENAME`）
  这种极端情况才保留 "没有可识别的官方软件源" 错误。

dnf/yum 按仓库块（`[section]`）解析：

- 只转换包含 `baseurl=`（含被注释的 `# baseurl=`）的块：解注释并重写官方主机，
  同时把 `mirrorlist=`/`metalink=` 行注释掉（前缀 `#lkit-mirror: `）；
- Fedora：`download.fedoraproject.org/pub/fedora` 与 `.../pub/epel` 映射到镜像；
- CentOS 7：`mirror.centos.org/centos`；CentOS Stream：`mirror.stream.centos.org`
  映射到镜像的 `centos-stream`；Rocky：`dl.rockylinux.org/$contentdir`；AlmaLinux：
  `repo.almalinux.org/almalinux`；
- 没有 `baseurl=` 的仓库块原样保留并计入跳过统计，避免把仓库改成空配置；
- 自定义主机 URL 不受影响；已识别的九个公共镜像（TUNA、阿里云、USTC、NJU、SJTU、
  ZJU、LZU、BFSU、HUST）之间可互转，选择其中一个时其余镜像的 URL 一并替换为所选
  镜像。

pacman 直接生成新的 `mirrorlist`：注释头说明来源，随后是单个选中的
`Server = https://<镜像>/archlinux/$repo/os/$arch`（官方源使用
`geo.mirror.pkgbuild.com`）。

`official` 把已识别的镜像主机 URL 恢复为官方主机（反向映射），用于换回官方源；
已在官方源或已处于所选镜像的重复执行不会修改文件，视为成功 no-op（打印
"nothing was changed" 提示，退出码 `0`）。

## 备份与恢复

执行换源前把受管源文件备份到 `/var/lib/lkit/mirror-backup/<family>/`（保留相对路径），
成功后才修改正式文件。每次实际修改都会覆盖上一份备份；no-op（已在目标状态）不会
触碰已有的备份，因此重复执行同一目标不会丢掉上一次保存的原源。

`--restore` 把备份的文件按原路径写回并删除备份目录。没有备份时返回错误提示先执行换源。
`--show` 打印当前受管源文件的路径与内容，`--list` 列出当前发行版可用的镜像。

## 权限与确认

修改软件源（换源与恢复）要求 root（euid 0）；`--list` 与 `--show` 只读，不需要 root。
交互终端中默认要求输入 `yes` 确认（与 `lkit update` 一致），`--yes` 或
`--non-interactive` 跳过确认。交互选择 Debian 镜像时，在确认前会额外询问是否同时
替换 security 仓库，默认保留官方（对应 `--replace-security`）。取消确认时输出提示并
不做任何修改，返回退出码 `1`。

换源成功后打印修改的文件数、备份路径与跳过的仓库数（适用时）；源文件已处于目标
镜像/官方状态时打印 "nothing was changed" 并以成功退出。apt 走兜底路径（CD 源转换
或合成新条目）时额外打印对应提示。命令级失败（发行版不支持、没有源文件、没有备份）
返回退出码 `1` 并打印原因。

## 控制台入口

交互控制台（裸 `lkit`）侧栏新增“Mirror（换源）”面板：进入面板时检测发行版并显示
主机摘要，提供十个镜像选项与“恢复备份的原软件源”动作；Enter 打开居中确认层，Debian
主机确认层内有一行默认不勾选的“同时替换 security 仓库”开关（Space/←/→ 切换，
也可点击），确认后在控制台内同步执行（与 CLI 相同的备份、重写与恢复语义），结果写入
底栏。面板不依赖 Landscape 安装状态，未安装或已安装均可使用。
