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
- Versions；
- Configuration；
- Services；
- Network；
- Diagnostics。

除 Overview 和 Install 外的面板在对应管理能力实现前明确显示不可用，不执行隐式操作。
后续功能继续加入相同侧栏外壳，不改变 CLI 子命令契约。

Install 面板提供版本、仓库类型、安装根目录、管理员用户名、密码、密码确认、service
manager 和网络接管选项；自定义仓库 URL 只在仓库类型为 `Custom HTTP` 时显示并接受输入。
版本默认 `latest`，也可直接编辑为精确 stable 版本；HTTP repository protocol v1 没有版本
目录接口，因此首版不伪造远端版本列表。密码和确认密码在界面中只显示等长 `*`，提交前
检查两次输入相同并复用 Landscape 密码复杂度规则；用户无需为控制台安装准备密码文件。
表单为当前选中项显示配置含义和影响；宽终端在表单右侧显示，窄终端空间允许时显示在
表单下方。

首次进入 Install 时，控制台在后台调用与 `lkit check` 相同的只读检查并在表单顶部显示
pass、warning、error 和 unknown 汇总，不阻塞按键与渲染。检查汇总是 Install 的第一个
焦点项；Enter 或 Right 展开按组排列的检查结果，显示检查值、非通过原因和处理建议，详情
中使用 Up/Down、PageUp/PageDown 滚动，Esc 收起。R 可随时重新检查。该结果用于部署前诊断，
不显示在 Overview：安装完成后 Landscape 自身占用的服务端口不应被当作部署前端口冲突。

表单在执行前使用与命令模式相同的版本、用户名、路径和仓库校验。开始安装后先退出
alternate screen、恢复终端，再把结构化 `Install` 请求交给共享命令分发。需要 systemd
托管时仍创建 operation unit；控制台不得启动另一个 lkit 进程或解析 CLI 文本输出。
inline 安装只在内存中传递控制台密码。systemd worker 需要跨进程传递时，在
`/run/lkit/operations` 创建 root-only `0600` 临时凭据文件，只把文件路径加入内部 worker
参数；密码不进入 argv、环境、request JSON、stdout/stderr 或展示事件。worker 正常完成、
失败或前端 Ctrl+C 成功停止 unit 后删除该文件；停止 worker 失败时与其他运行时现场一并
保留，避免正在运行的操作失去凭据。

## 输入与恢复

侧栏和表单使用方向键移动：Right 或 Enter 从侧栏进入面板，Left 从任意面板（包括
Install）返回侧栏；Install 使用 Up/Down 在检查汇总和表单字段间移动，Right 或 Enter/Space
切换枚举和开关。Tab 在侧栏与面板间切换，Enter 编辑或激活当前项。非编辑状态第一次 Esc
只进入等待状态，连续第二次 Esc 才打开居中的退出确认层；确认层使用 Enter 退出、Esc 取消。
第一次 Esc 后的任意其他按键取消等待并继续原操作。编辑状态的 Esc 只结束编辑，Ctrl+C 仍
立即退出。编辑状态支持普通字符、退格和终端 paste 事件，单字段最多接收 1024 个字符。
控制台底栏按当前焦点显示可用操作，右下角持续显示当前语言。非文本编辑和非退出
确认状态下按 `L` 在英文与中文之间即时切换，不需要退出或重启控制台。编辑字段时的
`l` 保持为普通输入字符。

切换语言后会重新读取安装状态；如果部署前检查已启动或完成，会用新语言重新执行检查，
避免保留旧语言的检查说明。切换后从控制台启动的命令和 systemd worker 继承新语言。

控制台的 RAII terminal guard 在正常退出和错误返回时关闭 raw mode、离开 alternate
screen 并显示光标。进程级 Ctrl+C guard 另外保存原始 termios；收到信号时恢复终端，覆盖
密码输入已经关闭 ECHO 或动态进度已经隐藏光标的场景。
