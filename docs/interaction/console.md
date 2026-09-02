# Ratatui 管理控制台

裸命令 `lkit` 是长期使用的交互管理入口。它进入 alternate screen，显示固定侧栏和当前
功能面板；带子命令的调用保持命令模式，不进入全屏控制台。

```text
lkit                              # Ratatui 管理控制台
lkit install --version 1.2.3 ...  # 命令模式
lkit --non-interactive install ... # 严格非交互命令模式
```

裸命令要求 stdin 和 stdout 都连接终端。终端不可用时返回失败并提示使用具体子命令；
`lkit --non-interactive` 未提供子命令属于参数使用错误。

## 控制台布局

侧栏固定包含：

- Overview：读取 lkit 地盘(`/root/.lkit/`)的安装状态并展示 landscape 根位置；
- Install：首次安装表单；
- Backup：备份列表、创建与恢复；
- Update：版本更新表单（仅已安装时可用）；
- Mirror：主机软件源换源面板（见[`lkit set-mirror`](../commands/mirror.md)）；
- Software：常用软件安装面板（见[`lkit software`](../commands/software.md)）；
- Reinit：重新初始化面板（仅已安装、systemd 且宿主网络服务已被接管时可用）。

检测到 Landscape 已安装时，Install 菜单（首次安装表单）在侧栏中置灰且不可选中，
Up/Down 导航会跳过它；面板仍可显示“已安装”提示。反之，未安装、非 root 或安装
状态不可读时 Update 菜单置灰且被导航跳过，面板显示不可用原因。卸载入口暂未在侧栏
启用（见下文 Uninstall 面板说明）。

Overview 面板以左右双栏展示两个服务(面板小于 52 列时回退为上下堆叠)：
左栏为 Landscape 安装状态与详情（版本、service manager、初始化、安装根），
右栏常驻显示 lkit 常驻服务小节：版本号（当前二进制版本，daemon 未运行时也显示）
与运行状态，运行中显示绿色"lkit 常驻服务：运行中"，未运行显示红色"lkit 常驻服务
未运行;请用 `lkit self install` 部署"。root 会话的安装与生命周期命令都委托给
daemon，未运行时应在进入控制台就可见，而不是填写完安装参数、退出控制台委托时才
失败。daemon 未运行且面板获得焦点时，右栏出现"`> [ 部署 lkit 常驻服务 ]`"动作行：
聚焦时反色高亮（黑底青字加粗），未聚焦时绿字加粗；Enter 打开居中确认层（说明将
注册并启动常驻服务），确认后在 TUI 内后台线程执行与 CLI `lkit self install`
相同的流程（root 检查、安装锁、systemd 注册与启动），部署期间显示进度弹层且按键
被忽略（Ctrl+C 仍退出），结果写入底栏；成功后状态行在下一帧自动变为运行中。确认层
内嵌三个导航单元：急救恢复码（flare psk）输入、二次确认输入与「开始部署」动作行，
方向键/Tab 在单元间移动（聚焦单元反色高亮）；Enter 在输入字段上进入编辑（编辑中
明文、其余时刻掩码，Enter/Esc 提交），直接输入字符也会进入编辑，在「开始部署」
动作行上才执行校验并启动部署——校验要求非空 psk 至少 12 个字符且与二次确认一致，
不满足则底栏提示并留在弹窗；两框留空由 daemon 首启自动生成。daemon 运行时右栏
提供"`> [ 查看急救恢复码 ]`"动作行，Enter/空格或点击弹出「查看/修改急救恢复码」
弹窗：psk 明文展示（供分发给恢复操作员），内嵌 psk 与二次确认两个输入框和「保存」
动作行，交互与部署确认层相同，保存时校验非空、至少 12 字符且两次一致后写回
`[flare]` 段（daemon 下一周期拾取）。控制台不另起 lkit
进程、不解析 CLI 文本输出。进入控制台时若 root 下 daemon 未运行，底栏提示行也
直接显示同样的警告。控制台 header 在品牌标题右侧展示两个状态徽标
（Landscape 安装状态——带主语如 `Landscape: installed`——与 lkit daemon 状态），
窄终端放不下时徽标全部隐藏；各页面样式验收标准见
[控制台样式验收标准](ui/README.md)。

Install 面板提供版本、仓库类型、安装根目录、管理员用户名、密码、密码确认和 service
manager 选项；自定义仓库 URL 只在仓库类型为 `Custom HTTP` 时显示并接受输入。
版本默认 `latest`，也可直接编辑为精确 stable 版本；HTTP repository protocol v1 没有版本
目录接口，因此首版不伪造远端版本列表。密码和确认密码在界面中只显示等长 `*`，提交前
检查两次输入相同并复用 Landscape 密码复杂度规则；用户无需为控制台安装准备密码文件。
表单为当前选中项显示配置含义和影响；宽终端在表单右侧显示，窄终端空间允许时显示在
表单下方。

Install 面板始终启用网络接管（开关暂隐藏，见代码中的 `TODO(network-takeover)`；处理完
不同发行版网络服务差异后恢复）。激活“开始安装”不会在表单内继续逐行询问，而是进入
无侧栏的全屏网络向导。向导依次进入 WAN 选择、WAN 配置面板、LAN 选择和 LAN DHCP
配置面板，最后显示计划摘要。

WAN 列表与默认网关一同识别：每个网卡显示发现顺序中的首个 IPv4 及该接口的首个默认网关；
没有默认网关时明确显示未发现。选中 WAN 后进入 WAN 配置面板：顶部两个 tab（静态 /
DHCP client）用 Left/Right 切换模式，按与 CLI 相同的发现顺序预填该 IPv4 和网关
（两项均存在时默认 Static，缺任一项默认 DHCP）；面板中部在静态模式下显示
IPv4 地址/CIDR 与默认网关两个可编辑字段（Up/Down 移动焦点，普通输入、paste 和
Backspace 编辑，Enter 提交字段并下移），DHCP 模式下显示 DHCP client 说明；
面板底部是“确认并继续”按钮（Up/Down 移动到后按 Enter 确认，静态模式会校验
地址与网关）。切换 tab 保留已填写的静态值。

LAN 列表只包含 WAN 以外的物理网卡，使用 Up/Down 移动、Space 多选、Enter 确认。LAN 可以
不选择，包括多网卡主机；空集合生成 WAN-only 计划，直接进入摘要，不创建 `br_lan` 或
LAN DHCP。选择至少一个 LAN 后进入单页 LAN DHCP 配置面板：管理地址、DHCP 地址池起始
和结束三个字段在同一页编辑（进入时按管理地址默认池预填起始/结束），底部“确认并继续”
按钮一次性校验并进入摘要。向导结束前显示计划摘要，列出 WAN 接口和 MAC、Static
IPv4/网关或 DHCP、LAN/WAN-only 模式、管理地址和 DHCP 范围（适用时），并明确所选 LAN
会清理 IPv4/IPv6 地址、未选择接口保持不变。Enter 确认摘要后才开始安装。

Esc 在非首页步骤返回上一步并保留已填写值；从 WAN 首页按 Esc 才打开“取消网络向导”确认层。
确认层使用 Enter 取消向导并返回 Install 表单，Esc 关闭确认层并继续向导，尚未开始安装。

Backup 面板在未安装、非 root 或安装状态不可读时只显示原因提示，不执行隐式操作。可用时
进入面板后在后台读取 `backups/` 下的 `.lkb` 列表：只读 32 字节 header 与 metadata JSON
（不读取归档体，不计算校验和，不做解包校验），因此切换面板几乎瞬时。列表顶部固定为
“创建备份”动作，下方按创建时间从新到旧排列备份；每行单行展示
（**备注排第一**，后跟 `backup_id + 创建时间 + 版本`；其他信息固定占位，备注按
剩余长度截断），完整备注在详情页查看；
结构性损坏（magic、header、metadata JSON 等）或权限不安全的条目标记为 invalid 且
不能打开或恢复，归档体损坏但 metadata 完好的条目在列表内视为可读，完整校验交给
详情页自动校验与恢复流程。Up/Down 选择，Enter 在“创建备份”上打开创建对话框、
在备份条目上打开 metadata 详情（与 `backup show` 相同的字段，**备注排第一**，
Up/Down 滚动）。进入详情即在后台执行与 `backup verify` 相同的完整校验（读文件 +
`verify_lkb` + 解包）并把结果显示在底栏，不阻塞查看；V 可随时手动重校验。R 在详情页
打开恢复确认层，但若最近一次校验失败（备份损坏）则打开损坏提示弹框而不进入恢复；
D 打开删除确认层（展示备份 ID、版本与“将永久删除”提示，Enter 删除、Esc 取消；删除在
控制台内同步执行——与 CLI 相同的根目录解析、安装锁与文件校验，成功后自动刷新列表），
Esc 返回列表。创建对话框显示 minimal scope 说明（将创建 minimal 配置级快照，包含
恢复所需的最小文件集），并提供备注输入行：普通字符、退格、paste，最多 256 个字符，
Enter 提交（走与 CLI 相同的备注校验，空备注直接创建）、Esc 取消。提交后创建在控制台
内进行：后台 worker 执行与 CLI 相同的完整流程（安装锁、中断事务恢复、配置导出、归档、
落盘自校验），居中弹窗按阶段显示“导出配置 / 归档 N/M 个文件（当前文件名）/ 落盘校验”
并带百分比 Gauge，完成自动刷新列表并显示创建的备份 ID，全程不退出控制台。恢复确认层
展示备份 ID 与版本、提示当前版本将被替换，并显示 minimal scope 数据损失警告（不含
SQLite 数据文件，数据库恢复后按备份配置重建；API token、日志和指标不包含，备份之后
产生的数据将丢失）。Enter 前必须通过完整校验：未校验先启动校验并提示“校验中”，
校验失败弹损坏提示框，只有校验通过才把结构化 `Restore` 请求交给共享命令分发并退出
alternate screen（systemd 模式仍委托 worker，不解析 CLI 文本输出）；该请求标记为
已确认（`--console-confirmed`），命令不再请求 `/dev/tty` 二次确认——worker 是独立
进程，无法读取 TUI 键盘输入，继续交互确认会阻塞。Esc 取消。

Update 面板提供与命令模式 `lkit update` 相同的交互语义：选择本次更新读取的仓库来源、
解析目标版本、比较当前版本并要求确认，确认后复用 switch 流水线执行。面板只读取
`config.toml`（与 `lkit update` 相同，从不创建、更新或删除该文件）：文件存在且有效时
仓库选项的首项是“当前来源（kind: location）”，其余为官方 GitHub、默认 HTTP 镜像和
自定义 HTTP 仓库；文件不存在时选项从官方 GitHub 开始；文件存在但损坏时面板显示错误
提示且不提供“当前来源”选项，用户仍可改用显式来源继续。面板顶部显示当前版本，下方为
目标版本（可编辑，默认 `latest`，与命令模式相同的 `TargetVersion` 校验）、仓库来源
（Up/Down 或 Left/Right 切换枚举）和自定义仓库 URL（仅自定义 HTTP 时显示），以及“开始
更新”动作。

激活“开始更新”后，面板在后台线程解析目标版本（与 CLI 相同的 provider、架构与
`latest`/显式版本解析；网络只读，零副作用），解析期间按键被忽略并显示“正在解析目标
版本…”。解析完成后按与命令模式相同的规则分支：目标低于当前版本时面板内显示降级错误
（不退出控制台）；目标与当前相同（已是最新）时面板内显示“已是最新版本 <X>”（不创建
事务、不下载、不持久化所选来源）；解析失败（网络、版本不存在等）时面板内显示错误并可
修改字段重试；只有目标高于当前版本时才显示居中确认层：`当前 <X> → 目标 <Y>`（Y 为
解析出的真实版本），并说明更新复用 switch 流水线（事务、`.lkb` 配置快照、systemd 托管、
健康检查与自动回滚）。Enter 确认后控制台把结构化 `Update` 请求（标记
`--console-confirmed`）交给共享命令分发并退出 alternate screen，systemd 模式下委托
worker，在无侧栏全屏更新页（标题“正在更新 Landscape”）显示下载、配置与服务阶段及
结果页；Esc 取消确认层并留在面板。`--console-confirmed` 使命令跳过 `/dev/tty` 的渠道
选择与 `yes` 确认，也不在 switch 流水线内做任何交互确认；面板按所选来源传递显式
`--repository`（官方 GitHub、默认镜像与自定义 URL 分别映射为 `--repository github`、
裸 `--repository` 与 `--repository <URL>`），“当前来源”不传该参数，由命令按
`config.toml` > 官方 GitHub 的规则解析，与命令模式选中“当前来源”的语义一致。

Uninstall 面板暂未在 TUI 中启用：侧栏只显示 Overview、Install、Backup、Update、Mirror、
Software 与 Reinit，`Menu::ALL` 中 `Self::Uninstall` 以注释保留，面板渲染、键处理与确认层
代码完整保留供重新启用（`TODO(uninstall-console)`）。卸载当前只能通过命令模式
`lkit uninstall` 使用；本段描述的是面板重新启用后的行为：面板在已安装、root 且安装
状态可读时可用。进入面板后展示当前版本、服务 manager 与运行状态摘要，并列出卸载数据
损失范围（数据库、API token、日志和指标不可逆删除）与保留物（lkit 地盘的
`config.toml`、`backups/`、`transactions/`）；检测到网络接管特征（宿主网络服务被 stop/disable/mask）
时追加醒目警告，说明卸载不会恢复宿主网络服务。面板提供“开始卸载”动作，激活后打开居中
确认层，展示上述摘要并明确要求确认；Enter 确认后控制台把结构化 `Uninstall` 请求
（标记 `--console-confirmed`）交给共享命令分发并退出 alternate screen，systemd 模式
委托 worker，在无侧栏全屏卸载页显示准备、停止服务、注销与清理阶段及结果页，成功时展示
保护备份 ID 与保留物清单；Esc 取消确认层并留在面板。卸载确认层与 restore 一样承担
全部确认，命令不再请求 `/dev/tty` 二次确认。

Mirror 面板（换源）不依赖 Landscape 安装状态，未安装或已安装均可使用。进入面板时在
后台检测当前发行版并显示主机摘要（发行版家族与软件包管理器），下方列出四个镜像选项
（清华 TUNA、阿里云、中科大 USTC、官方源）与“恢复备份的原软件源”动作行，Up/Down
移动焦点。Enter 打开居中确认层：换源确认层说明将切换的家族与镜像、当前源会先备份且
可在此恢复；恢复确认层说明备份内容将替换当前镜像源文件。确认后在控制台内同步执行
与 CLI 相同的备份、重写或恢复流程（非 root 时底栏显示权限错误，不 panic），结果写入
底栏；Esc 关闭确认层。面板不退出 alternate screen，也不委托 systemd worker。

Software 面板（常用软件）与 Mirror 面板一样不依赖 Landscape 安装状态。进入面板时在
后台检测当前发行版并列出软件及其安装状态（当前为 Docker 一项，进入面板即默认选中并
高亮），Up/Down 移动焦点。
对未安装的软件按 Enter 打开居中确认层：显示安装来源（官方仓库、阿里云、清华 TUNA、
中科大 USTC，当前为官方），Space/Left/Right 循环切换来源，Enter 确认、Esc 取消；
已安装的软件按 Enter 只显示“已安装”提示。确认后安装不退出 alternate screen：后台
线程执行与 CLI 相同的完整流程（依赖与仓库准备、软件包安装、服务启用与 `docker info`
验证），居中弹窗按阶段显示“准备软件源 / 安装软件包 / 启动服务”并带 Gauge 进度，
弹窗内底部醒目提示“Esc 取消安装”（黄字加粗）。安装期间按 Esc 打开取消确认层
（说明将终止正在运行的软件包管理器命令、已写入的源文件保留下次覆盖）：Enter 确认后
置位取消标志，worker 终止子进程并返回取消提示，面板恢复可重新选择来源；Esc 关闭确认层
继续安装。安装的软件包子进程设置 PDEATHSIG，Ctrl+C 退出控制台后自动终止，不留残留；
完成
后自动刷新软件状态并把结果写入底栏（Ctrl+C 仍退出控制台）。
非 root 或发行版检测失败时确认 Enter 不启动安装，底栏显示权限或检测错误。

Reinit 面板只对已安装、`service.manager == systemd` 且宿主网络服务已被接管
（NetworkManager、`networking.service`、firewalld、systemd-resolved 被 stop/disable/mask）
的安装可用，其余情况面板显示不可用原因且菜单被导航跳过；CLI `lkit reinit` 与
[`lkit install --takeover-network`](install.md) 的前置条件一致。面板顶部展示当前版本与
服务摘要，并说明 reinit 会清空除新网络计划与新凭据外的全部配置（DNS 规则、已登记设备、
证书、DDNS 任务等由 Landscape 重建数据库）。聚焦面板时“开始 reinit”动作行显示 `>`
光标标记并高亮，Enter 进入与 Install 相同的全屏网络向导；向导确认后回到面板的凭据
步骤：管理员用户名、密码与密码确认三个字段（密码以等长 `*` 显示，提交前检查两次输入
一致并复用密码复杂度规则），下方显示新计划摘要（WAN 与 LAN，未选 LAN 时显示
无）与“重新初始化
Landscape”动作。动作行校验通过后打开居中确认层，说明清空范围、保护 `.lkb` 备份与
确认窗口（提交等待 `lkit network confirm`，会话可能断开）；Enter 确认后控制台把结构化
`Reinit` 请求（标记 `--console-confirmed` 与 `--yes`，密码与网络计划经凭据文件和计划
文件传入 worker）交给共享命令分发并退出 alternate screen，systemd 模式委托 worker，
在无侧栏全屏重新初始化页显示准备、停止服务、激活与健康检查阶段及结果页，成功进入
待确认状态后由 `lkit network confirm`/`rollback` 收尾；与 Install 结果页相同，
reinit 结果页检测到待确认网络接管时同样提供“确认接管”入口（Enter 打开确认层、
Enter 确认后内联执行 `lkit network confirm`，兜底语句与 Install 一致）；Esc 取消确认层并留在面板。

首次进入 Install 时，控制台在后台调用与 `lkit check` 相同的只读检查并在表单顶部显示
pass、warning、error 和 unknown 汇总，不阻塞按键与渲染。检查汇总是 Install 的第一个
焦点项；Enter 或 Right 展开按组排列的检查结果，显示检查值、非通过原因和处理建议，详情
中使用 Up/Down、PageUp/PageDown 滚动，Esc 收起。R 可随时重新检查。

检查包含 lkit 常驻服务项（与 `lkit check` 相同，见 [`check`](../check.md)）：root 下
daemon 未运行时报告 `error` 并建议 `lkit self install`，未部署 daemon 前无法进入安装
表单；非 root 会话只报告 `warning`，不阻断。阻断弹框内直接提供“[ 部署 lkit 常驻
服务 ]”按钮（按 `D` 或点击，按钮常显选中态）：打开与 Overview 相同的部署确认弹窗
（内嵌急救恢复码输入与二次确认，交互见 Overview 小节），确认后在 TUI 内后台执行
`lkit self install`，完成后预检自动
重跑、报告更新后表单门禁自然放行；弹框内长文本自动换行不截断。部署成功后（无论从
Overview 动作行还是阻断弹框发起）预检都会自动重跑，不需要手动按 `R` 刷新过期的
daemon 检查结果。

检查尚未运行或仍在运行时，用户不能离开检查汇总进入表单，焦点保持在汇总并显示等待状态。
检查完成后，Pass 和 warning 都允许进入表单；warning 继续显示其风险提示。Error、unknown
或检查 worker 失败时，任何进入表单、开始安装或进入网络向导的路径都会被阻止，并显示居中
处理弹窗。弹窗显示阻断项和建议，Enter 查看详情、Esc 关闭、R 重新检查，没有强制跳过入口。
进入表单后重新检查若变为阻断状态，“开始安装”和网络向导入口在激活前同样应用该门禁。
该结果用于部署前诊断，不显示在 Overview：安装完成后 Landscape 自身占用的服务端口不应被
当作部署前端口冲突。

安装根存在未完成网络接管（`awaiting_network_confirmation`、`finalizing`、
`rolling_back`）时，启动快照进入待确认状态，TUI 直接显示阻塞屏而不渲染菜单：展示事务
ID、阶段、管理地址（DHCP 租约时显示占位）、确认截止时间和自动回滚提示，并提供“稍后”
与“确认执行”两个选项。自动回滚提示行过长时在弹窗内自动换行，不做截断。阻塞屏不启动环境检查或备份轮询，Install 菜单不可进入；“稍后”
（默认选项，Enter/Esc/Ctrl+C）退出 TUI 回 shell，重新进入仍显示阻塞屏；“确认执行”
（↑/↓ 或 Tab 选择后 Enter）退出 TUI 并按现状命令行语义内联运行 `lkit network confirm`
（普通终端输出，无全屏页，不限制 SSH 会话来源）。`rolling_back` 阶段只提供“稍后”。

表单在执行前使用与命令模式相同的版本、用户名、路径和仓库校验。激活“开始安装”与网络
向导确认摘要时（以及检查结果可能过时的情况下）都会重新检查委托前置条件：root 下
daemon 未运行时不退出控制台，留在面板内提示“lkit 常驻服务未运行;请用 `lkit self
install` 部署”，而不是在用户填写完所有安装参数、退出控制台委托时才报错。委托前置
条件还包括 daemon 能否 spawn 自己的 worker 子进程：daemon 以 `current_exe()` 的路径
启动 worker，若该可执行文件已被删除或替换（spawn 会报 `ENOENT`，且 daemon 只把错误
写进 journald，前端会永远等待结果），进入控制台与激活安装都会在 TUI 内提示“lkit
常驻服务无法启动工作命令：其可执行文件已被删除或替换；请恢复该文件并重启常驻服务”，
命令模式则由 `delegate()` 以退出码 `2` 拒绝，不再无限等待。网络向导完成后把结构化
`Install` 请求交给共享命令分发；systemd worker 在同一个无侧栏全屏安装页显示下载、配置、
网络和服务阶段。下载阶段支持 Ctrl+C 停止，Esc 打开停止确认；进入配置、网络或服务阶段
后停止请求只显示提示并继续，成功、失败或取消结果页保持到 Ctrl+C。结果页的 Output 面板
会把网络接管“等待确认”、“重新连接后运行 `lkit network confirm`”以及“未在期限内确认将
自动回滚”的提示行用黄底黑字加粗醒目标出。安装（及 reinit）完成且存在待确认的网络接管
事务时，结果页底栏显示 `Enter 确认接管  Ctrl+C 关闭`：Enter 打开居中确认层（说明确认后
网络切换至 Landscape 托管计划、当前会话可能断开，并展示兜底语句——确认失败或会话中断
时用管理地址重连后运行 `lkit network confirm`，未在期限内确认将自动回滚
（`lkit network rollback`）），Enter 确认后退出全屏页并把 `network confirm` 委托给
daemon 执行（与手工命令同一条路径）：确认会切换托管网络，发起会话可能因此断开——
委托后即使前端进程消失，daemon 也独立完成提交，事务不会停在半提交状态；重连后重新
进入 `lkit` 即回到主页。Esc 返回结果页。关闭结果页后（以及
命令模式委托安装结束时），lkit 会在普通终端再输出一次明确的结果提示：成功打印
`install: installation complete`（或 `安装完成`），失败打印包含退出码的提示，避免用户在
没有全屏结果页或流式输出被忽略时继续等待。控制台不得启动另一个 lkit
进程或解析 CLI 文本输出。

全屏页标题按操作显示（“正在安装/切换/更新/修复/恢复 Landscape”等），每个委托操作
一个完全独立的页面组件（`src/interaction/presentation/screens/` 下每个操作一个文件），
各自维护完整的布局、进行中标题、完成/失败/取消结果页标题（如恢复完成显示
“恢复完成/Restore complete”，不复用安装文案）、结果状态框文案与底栏提示；将来某个
操作需要不同布局时只改它自己的文件。restore 等没有字节下载的委托操作使用步骤进度条：
worker 在准备、停止服务、激活、初始化与健康检查各阶段发送阶段与步骤事件（如 `2/4`），
全屏页以百分比 Gauge 显示；下载型操作（install）仍显示字节进度条。
inline 安装只在内存中传递控制台密码。systemd worker 需要跨进程传递时，在
`/run/lkit/operations` 创建 root-only `0600` 临时凭据文件，只把文件路径加入内部 worker
参数；密码不进入 argv、环境、request JSON、stdout/stderr 或展示事件。worker 正常完成、
失败或前端 Ctrl+C 成功停止 unit 后删除该文件；停止 worker 失败时与其他运行时现场一并
保留，避免正在运行的操作失去凭据。网络向导生成的结构化计划也使用同目录下独立的
root-only `0600` JSON 文件传入 worker，并在相同生命周期内删除；普通 CLI 用户不直接提供
该内部文件。

## 输入与恢复

支持鼠标（终端须支持 xterm 鼠标序列；进入 alternate screen 时启用鼠标捕获，退出时关闭）：
左键点击侧栏菜单项切换面板，点击面板内的检查汇总、表单字段、备份行、更新行与动作行
等价于先用方向键移动到该项再按 Enter（可编辑字段直接进入编辑，枚举项向前切换，动作项
按原确认流程执行）；确认层内点击弹窗区域等价于 Enter、点击弹窗外等价于 Esc，输入框
（如备份备注）弹窗内点击不触发动作。网络向导中可点击 WAN/LAN 列表行、模式 tab 和字段；
WAN/LAN 配置页其余区域点击等价于 Enter 继续，取消确认层仍按“弹窗内 Enter、弹窗外 Esc”。
阻塞接管屏的两个选项行可直接点击选中/执行。右键任意位置等价于 Esc；鼠标滚轮在展开的
检查详情和备份详情页滚动。所有鼠标动作都复用键盘语义，行为和快捷键一致。

侧栏和表单使用方向键移动：Right 或 Enter 从侧栏进入面板，Esc 从任意面板（包括
Install）返回侧栏菜单选择；Left 与 Right 一同作为面板内组件切换（Install 与 Update
的仓库枚举、软件来源与镜像确认层内的循环切换等）。Install 使用 Up/Down 在检查汇总和
表单字段间移动，Right/Enter/Space 切换枚举和开关。Tab 在侧栏与面板间切换，Enter 编辑
或激活当前项。退出确认只在导航层生效：非编辑状态在导航层第一次 Esc 只进入等待状态，
连续第二次 Esc 才打开居中的退出确认层；确认层使用 Enter 退出、Esc 取消。第一次 Esc
后的任意其他按键取消等待并继续原操作。编辑状态的 Esc 只结束编辑，Ctrl+C 仍
立即退出。编辑状态支持普通字符、退格和终端 paste 事件，单字段最多接收 1024 个字符。
焦点位于右侧面板时，面板标题带有 `> ` 前缀并使用高亮边框；Install 的检查汇总和表单当前项
也显示固定宽度的 `> ` 标记，保证不支持 truecolor 的终端仍能看见当前焦点。
控制台底栏按当前焦点显示可用操作，并始终把 `Ctrl+C Exit` 或 `Ctrl+C 退出` 放在操作提示
最前面；展开检查详情时同时显示 `Esc Close` 或 `Esc 关闭`，明确区分收起详情和退出整个
控制台。底栏高度随内容动态：状态行与操作提示行各至少一行，超长自动换行而不是截断
（窄终端下英文长提示可能换行到第二行），不留空行。右下角语言指示在可切换时显示
**目标语言**（以目标语言自身文字书写，如英文界面下的 `[L] Switch to 中文 (zh)`），
按 `L` 或点击该指示即切换到所示目标，所见即所得；文本编辑等不可切换状态退回显示
当前语言（如 `Language: English (en)`）。编辑字段时的
`l` 保持为普通输入字符。各页面样式验收标准见 [控制台样式验收标准](ui/README.md)。

切换语言后会重新读取安装状态；如果部署前检查已启动或完成，会用新语言重新执行检查，
避免保留旧语言的检查说明。切换后从控制台启动的命令和 systemd worker 继承新语言。
切换同时原子写回 `config.toml` 的 `[ui] language`（见[语言预设](../deployment/config.md)），
下次会话沿用；写回失败（如配置损坏）时显示提示，本次会话的切换不受影响。

控制台的 RAII terminal guard 在正常退出和错误返回时关闭 raw mode、离开 alternate
screen 并显示光标。进入 alternate screen 时先显式清屏并回到左上角，离开时先清屏再
退出，避免 VMware 控制台等不会在切换 alternate screen 时清空缓冲区的终端残留上一
帧符号。进程级 Ctrl+C guard 另外保存原始 termios；收到信号时恢复终端，覆盖
密码输入已经关闭 ECHO 或动态进度已经隐藏光标的场景。
