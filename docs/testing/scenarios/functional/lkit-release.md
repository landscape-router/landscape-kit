# lkit 自发布与引导安装

## LKR-01

**stable tag 只在版本一致且双架构构建成功后创建 GitHub Release**

- 测试层：GitHub Actions、自发布配置检查
- 状态：`部分覆盖`
- 证据：[lkit 自发布规范](../../../release/lkit.md)、[发布 workflow](../../../../.github/workflows/release-lkit.yml)
- 缺口：需要首次真实 tag 发布验证 GitHub Release 写入链路。

## LKR-02

**分发二进制版本、ELF 架构、链接方式、strip 状态和体积预算均合法**

- 测试层：双架构 GitHub Actions、Rust 单元
- 状态：`已覆盖`
- 证据：[lkit 自发布规范](../../../release/lkit.md)、[发布 workflow](../../../../.github/workflows/release-lkit.yml)

## LKR-03

**安装器按宿主架构选择资产并在校验后原子替换 lkit**

- 测试层：Shell 功能测试
- 状态：`已覆盖`
- 证据：[安装器测试](../../../../scripts/test-install-lkit.sh)、[安装入口](../../../release/lkit.md#安装入口)

## LKR-04

**一条管道命令安装 lkit 并进入既有 Landscape 安装流程**

- 测试层：Shell 功能测试、生产 Release smoke
- 状态：`部分覆盖`
- 证据：[安装器测试](../../../../scripts/test-install-lkit.sh)、[安装入口](../../../release/lkit.md#安装入口)
- 缺口：首次真实 Release 后需要在公开下载地址完成 x86_64 和 aarch64 smoke。
