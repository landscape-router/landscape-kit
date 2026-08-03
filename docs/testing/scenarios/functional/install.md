# 首次安装场景

## INS-01

**从 HTTP/RustFS 仓库安装显式版本**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[首次安装场景](../lifecycle.md#首次安装)、[完整 CLI E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)

## INS-02

**不指定版本时解析 stable 并安装 latest**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[latest 场景](../extended.md#s7-latest-通道安装)

## INS-03

**从默认 GitHub provider 完成首次安装**

- 测试层：Rust repository 测试
- 状态：`部分覆盖`
- 说明：provider 与下载逻辑分别覆盖，缺少完整 CLI 首装链路；见 [`lkit install`](../../../commands/install.md)。

## INS-04

**`--service-manager systemd` 安装后初始化、启动并验证服务**

- 测试层：CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[首次安装断言](../lifecycle.md#首次安装)

## INS-05

**`--service-manager none` 安装后提交 pending 状态且不启动服务**

- 测试层：Rust workflow、Docker E2E
- 状态：`已覆盖`
- 证据：[service manager 迁移前置场景](../extended.md#s6-服务管理器迁移none--systemd)

## INS-06

**未指定 manager 时根据 systemd 可用性自动选择**

- 测试层：Rust workflow
- 状态：`已覆盖`
- 证据：Rust workflow 直接覆盖 Auto 在 systemd 可用时选择 systemd、明确非 systemd 时
  选择 none，以及 systemd 环境损坏时拒绝；见[首次安装语义](../../../commands/install.md)。

## INS-07

**x86_64 和 aarch64 选择并启动各自发布资产**

- 测试层：双架构 Docker E2E
- 状态：`已覆盖`
- 证据：[Docker CI 边界](../../docker-e2e.md#本地运行)

## INS-08

**密码、初始化文件和 API token 满足输入校验与文件权限要求**

- 测试层：Rust 单元、CLI fixture E2E、Docker E2E
- 状态：`已覆盖`
- 证据：[初始化与凭据](../../../interaction/initialization-and-credentials.md)、[首次安装场景](../lifecycle.md#首次安装)
- 边界：端到端保密扫描单列在 `SEC-02`。

## INS-09

**下载、压缩包校验或启动健康检查失败时清理首次安装现场**

- 测试层：Rust workflow、CLI fixture E2E
- 状态：`已覆盖`
- 证据：[首次安装失败](../../../workflows/lifecycle.md#首次安装失败)、[失败清理 E2E](../../../../crates/lkit-cli/tests/install_fixture_e2e.rs)
- 说明：Rust workflow 分别注入资产 404、损坏后端 zstd 和非法 static zip，并断言发布
  临时目录、目标 release、current、初始化文件和成功 state 均不存在且事务终结；CLI
  fixture E2E 另行验证启动健康检查失败后的服务、unit 与 resolv.conf 恢复。

## INS-10

**未知非空目录、危险软链接、已有不可信 release 或 foreign unit 阻断安装**

- 测试层：Rust 单元与 workflow
- 状态：`已覆盖`
- 证据：[验收标准](../../../acceptance.md#安装状态与路径)

## INS-11

**可复用的可信 release 不重复下载或覆盖**

- 测试层：Rust workflow
- 状态：`部分覆盖`
- 说明：可信目录和冲突规则已有低层测试，缺少完整 CLI 场景；见 [下载与发布目录](../../../repository.md#下载与发布目录)。

## INS-12

**首次安装在提交前中断，下次调用清理未提交现场**

- 测试层：Rust 事务测试
- 状态：`已覆盖`
- 证据：[事务恢复](../../../deployment/transactions-and-recovery.md)
