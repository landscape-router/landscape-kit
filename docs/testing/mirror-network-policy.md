# CI 公共镜像联网策略

## 普通分支与 PR

镜像回归使用临时目录、命令替身和纯函数验证 URL、配置改写、备份恢复、可用性探测目标
及失败保护，不向公共镜像站发送探测、索引刷新或软件包下载请求。

真实 Docker/Alpine 软件安装测试仍联网，但使用官方仓库。构建依赖、容器镜像和 GitHub
制品下载不属于离线测试承诺。普通 Alpine 流程不切换到阿里云；虚拟机本身不访问外网。
本策略限制的是 lkit 主动选择、探测 TUNA／阿里云等公共镜像的行为；官方仓库的 CDN
或发行版 mirrorlist 仍可能自行分流，不对最终物理站点做网络审计。

## 专用分支

审阅待测提交后执行：

```sh
git push origin HEAD:ci-mirror
```

`test-docker-mirrors.yml` 只监听该分支的 push，没有 PR、定时或 workflow_dispatch 入口，
不设置路径过滤以免同步待测提交时漏测。两个架构串行执行；完成后通过 reusable workflow
调用 Alpine 镜像实测。该调用显式传入 `allow_mirror_network: true`。普通 Alpine 工作流
忽略该分支的直接 push，避免重复执行；其他触发方式的该输入默认为 false。

镜像实测会探测站点及刷新索引，因此只应在需要验证公网兼容性时推送，不跟随每次开发
提交同步。新推送取消同分支旧任务。Alpine 在镜像索引断言后恢复官方仓库，再安装真实
软件包，以免把后续大量包下载转给公共镜像站。

## 脚本 guard

`scripts/lib/mirror-network-guard.sh` 要求以下四项同时成立：

- `LKIT_ALLOW_MIRROR_NETWORK=1`；
- `GITHUB_ACTIONS=true`；
- `GITHUB_EVENT_NAME=push`；
- `GITHUB_REF=refs/heads/ci-mirror`。

宿主与换源容器入口均在任何 Docker／包管理操作前检查；Alpine 仅在显式申请公共镜像
模式时检查，其余情况走官方源。标记与 CI 元数据显式传入容器，guard 文件只读挂载。
复用工作流也在准备环境前校验公共镜像模式的申请，不允许其他分支或手动 dispatch
通过传入 true 来启用。

这是防误调用的操作策略，不是隔离恶意代码的网络防火墙。不要在本地伪造 CI 元数据
运行系统级测试，也不要把其他工作流的公共镜像请求绕过这些入口。

## 验证

`bash scripts/test-mirror-network-guard.sh` 使用纯 shell 函数验证 14 种许可／事件／ref
组合，不启动 Docker、不访问网络。普通 Rust CI 显式运行该测试和镜像离线单测。
实际镜像兼容性必须由 `ci-mirror` 的 push 验证，静态检查通过不代表实测已运行。
