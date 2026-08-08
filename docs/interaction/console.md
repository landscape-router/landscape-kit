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

首版侧栏固定包含：

- Overview：读取默认或 `LKIT_INSTALL_DIR` 指定根目录的安装状态；
- Install：首次安装表单；
- Backup：备份列表、创建与恢复；
- Versions；
- Configuration；
- Services；
- Network；
- Diagnostics。

除 Overview、Install 和 Backup 外的面板在对应管理能力实现前明确显示不可用，不执行隐式
操作。后续功能继续加入相同侧栏外壳，不改变 CLI 子命令契约。

Install 面板提供版本、仓库类型、安装根目录、管理员用户名、密码、密码确认、service
manager 和网络接管选项；自定义仓库 URL 只在仓库类型为 `Custom HTTP` 时显示并接受输入。
版本默认 `latest`，也可直接编辑为精确 stable 版本；HTTP repository protocol v1 没有版本
目录接口，因此首版不伪造远端版本列表。密码和确认密码在界面中只显示等长 `*`，提交前
检查两次输入相同并复用 Landscape 密码复杂度规则；用户无需为控制台安装准备密码文件。
表单为当前选中项显示配置含义和影响；宽终端在表单右侧显示，窄终端空间允许时显示在
表单下方。

启用网络接管后，激活“开始安装”不会在表单内继续逐行询问，而是进入无侧栏的全屏网络
向导。向导依次选择 WAN、WAN IPv4 模式、LAN 和 LAN 配置。WAN 列表与默认网关一同识别：
每个网卡显示发现顺序中的首个 IPv4 及该接口的首个默认网关；没有默认网关时明确显示未发现。
选中 WAN 后，向导按与 CLI 相同的发现顺序预填该 IPv4 和网关：两项均存在时默认 Static，
缺任一时默认 DHCP。Static 与 DHCP 模式用 Left/Right 选择、Enter 确认，不使用 Space
切换。Static 的地址/CIDR 和默认网关在同一页填写，Up/Down 切换字段，普通输入、paste 和
Backspace 编辑当前字段；两个字段均通过校验后，Enter 进入 LAN 选择。

LAN 列表只包含 WAN 以外的物理网卡，使用 Up/Down 移动、Space 多选、Enter 确认。LAN 可以
不选择，包括多网卡主机；空集合生成 WAN-only 计划，不创建 `br_lan` 或 LAN DHCP。选择 LAN
后继续填写管理地址和 DHCP 范围。向导结束前显示计划摘要，列出 WAN 接口和 MAC、Static
IPv4/网关或 DHCP、LAN/WAN-only 模式、管理地址和 DHCP 范围（适用时），并明确所选 LAN
会清理 IPv4/IPv6 地址、未选择接口保持不变。Enter 确认摘要后才开始安装。

Esc 在非首页步骤返回上一步并保留已填写值；从 WAN 首页按 Esc 才打开“取消网络向导”确认层。
确认层使用 Enter 取消向导并返回 Install 表单，Esc 关闭确认层并继续向导，尚未开始安装。

Backup 面板在未安装、非 root 或安装状态不可读时只显示原因提示，不执行隐式操作。可用时
进入面板后在后台执行与 `lkit backup list` 相同的解析和完整校验（含归档解包），列表顶部
固定为“创建备份”动作，下方按创建时间从新到旧排列备份；损坏或权限不安全的条目标记为
invalid 且不能打开或恢复。Up/Down 选择，Enter 在“创建备份”上打开创建对话框、在备份条目上
打开 metadata 详情（与 `backup show` 相同的字段，Up/Down 滚动），详情页 V 在后台执行与
`backup verify` 相同的完整校验并把结果显示在底栏，R 打开恢复确认层，Esc 返回列表。创建
对话框显示 minimal scope 说明（不含 SQLite 数据文件、API token、日志和指标，需从运行中
实例导出配置），并提供备注输入行：普通字符、退格、paste，最多 256 个字符，Enter 提交
（走与 CLI 相同的备注校验，空备注直接创建，不带 `--remark`）、Esc 取消。恢复确认层展示
备份 ID 与版本、提示当前版本将被替换，并显示 minimal scope 数据损失警告（不含 SQLite
数据文件，数据库恢复后按备份配置重建；API token、日志和指标不包含，备份之后产生的数据
将丢失）。Enter 确认后控制台把结构化 `Restore` 请求交给共享命令分发并退出 alternate
screen（systemd 模式仍委托 worker，不解析 CLI 文本输出）；该请求标记为已确认
（`--console-confirmed`），命令不再请求 `/dev/tty` 二次确认——worker 是独立进程，无法
读取 TUI 键盘输入，继续交互确认会阻塞。Esc 取消。

首次进入 Install 时，控制台在后台调用与 `lkit check` 相同的只读检查并在表单顶部显示
pass、warning、error 和 unknown 汇总，不阻塞按键与渲染。检查汇总是 Install 的第一个
焦点项；Enter 或 Right 展开按组排列的检查结果，显示检查值、非通过原因和处理建议，详情
中使用 Up/Down、PageUp/PageDown 滚动，Esc 收起。R 可随时重新检查。

检查尚未运行或仍在运行时，用户不能离开检查汇总进入表单，焦点保持在汇总并显示等待状态。
检查完成后，Pass 和 warning 都允许进入表单；warning 继续显示其风险提示。Error、unknown
或检查 worker 失败时，任何进入表单、开始安装或进入网络向导的路径都会被阻止，并显示居中
处理弹窗。弹窗显示阻断项和建议，Enter 查看详情、Esc 关闭、R 重新检查，没有强制跳过入口。
进入表单后重新检查若变为阻断状态，“开始安装”和网络向导入口在激活前同样应用该门禁。
该结果用于部署前诊断，不显示在 Overview：安装完成后 Landscape 自身占用的服务端口不应被
当作部署前端口冲突。

表单在执行前使用与命令模式相同的版本、用户名、路径和仓库校验。网络向导完成后把结构化
`Install` 请求交给共享命令分发；systemd worker 在同一个无侧栏全屏安装页显示下载、配置、
网络和服务阶段。下载阶段支持 Ctrl+C 停止，Esc 打开停止确认；进入配置、网络或服务阶段
后停止请求只显示提示并继续，成功、失败或取消结果页保持到 Ctrl+C。结果页的 Output 面板
会把网络接管“等待确认”、“重新连接后运行 `lkit network confirm`”以及“未在期限内确认将
自动回滚”的提示行用黄底黑字加粗醒目标出。关闭结果页后（以及
命令模式委托安装结束时），lkit 会在普通终端再输出一次明确的结果提示：成功打印
`install: installation complete`（或 `安装完成`），失败打印包含退出码的提示，避免用户在
没有全屏结果页或流式输出被忽略时继续等待。控制台不得启动另一个 lkit
进程或解析 CLI 文本输出。

全屏页标题按操作显示（“正在安装/切换/更新/修复/恢复 Landscape”等）。restore 等没有字节
下载的委托操作使用步骤进度条：worker 在准备、停止服务、激活、初始化与健康检查各阶段
发送阶段与步骤事件（如 `2/4`），全屏页以百分比 Gauge 显示；下载型操作（install）仍显示
字节进度条。
inline 安装只在内存中传递控制台密码。systemd worker 需要跨进程传递时，在
`/run/lkit/operations` 创建 root-only `0600` 临时凭据文件，只把文件路径加入内部 worker
参数；密码不进入 argv、环境、request JSON、stdout/stderr 或展示事件。worker 正常完成、
失败或前端 Ctrl+C 成功停止 unit 后删除该文件；停止 worker 失败时与其他运行时现场一并
保留，避免正在运行的操作失去凭据。网络向导生成的结构化计划也使用同目录下独立的
root-only `0600` JSON 文件传入 worker，并在相同生命周期内删除；普通 CLI 用户不直接提供
该内部文件。

## 输入与恢复

侧栏和表单使用方向键移动：Right 或 Enter 从侧栏进入面板，Left 从任意面板（包括
Install）返回侧栏；Install 使用 Up/Down 在检查汇总和表单字段间移动，Right 或 Enter/Space
切换枚举和开关。Tab 在侧栏与面板间切换，Enter 编辑或激活当前项。非编辑状态第一次 Esc
只进入等待状态，连续第二次 Esc 才打开居中的退出确认层；确认层使用 Enter 退出、Esc 取消。
第一次 Esc 后的任意其他按键取消等待并继续原操作。编辑状态的 Esc 只结束编辑，Ctrl+C 仍
立即退出。编辑状态支持普通字符、退格和终端 paste 事件，单字段最多接收 1024 个字符。
焦点位于右侧面板时，面板标题带有 `> ` 前缀并使用高亮边框；Install 的检查汇总和表单当前项
也显示固定宽度的 `> ` 标记，保证不支持 truecolor 的终端仍能看见当前焦点。
控制台底栏按当前焦点显示可用操作，并始终把 `Ctrl+C Exit` 或 `Ctrl+C 退出` 放在操作提示
最前面；展开检查详情时同时显示 `Esc Close` 或 `Esc 关闭`，明确区分收起详情和退出整个
控制台。右下角持续显示当前语言。非文本编辑和非退出
确认状态下按 `L` 在英文与中文之间即时切换，不需要退出或重启控制台。编辑字段时的
`l` 保持为普通输入字符。

切换语言后会重新读取安装状态；如果部署前检查已启动或完成，会用新语言重新执行检查，
避免保留旧语言的检查说明。切换后从控制台启动的命令和 systemd worker 继承新语言。

控制台的 RAII terminal guard 在正常退出和错误返回时关闭 raw mode、离开 alternate
screen 并显示光标。进入 alternate screen 时先显式清屏并回到左上角，离开时先清屏再
退出，避免 VMware 控制台等不会在切换 alternate screen 时清空缓冲区的终端残留上一
帧符号。进程级 Ctrl+C guard 另外保存原始 termios；收到信号时恢复终端，覆盖
密码输入已经关闭 ECHO 或动态进度已经隐藏光标的场景。
