# Release 发布与仓库场景

## PUB-01

**手动镜像指定的官方 stable Release 到 RustFS**

- 测试层：手动 GitHub Actions
- 状态：`部分覆盖`
- 证据：[仓库发布流程](../../../repository.md#第三方仓库发布流程)、[发布 workflow](../../../../.github/workflows/publish-landscape-mirror.yml)
- 缺口：缺少生产公开链路的发布后验证。

## PUB-02

**首次发布创建 `repository.json`、version manifest 和 stable 指针**

- 测试层：RustFS 发布集成
- 状态：`已覆盖`
- 证据：[发布集成脚本](../../../../scripts/test-publish-http-repository.sh)

## PUB-03

**发布更高版本后 stable 单向前进**

- 测试层：RustFS 发布集成
- 状态：`已覆盖`
- 证据：[RustFS 容器集成](../../../repository.md#rustfs-容器集成测试)

## PUB-04

**发布历史版本时保留当前 stable**

- 测试层：RustFS 发布集成
- 状态：`已覆盖`
- 证据：[RustFS 容器集成](../../../repository.md#rustfs-容器集成测试)

## PUB-05

**重复发布同一版本时拒绝覆盖不可变对象**

- 测试层：RustFS 发布集成
- 状态：`已覆盖`
- 证据：[发布集成脚本](../../../../scripts/test-publish-http-repository.sh)

## PUB-06

**资产缺失或上传失败时不提交 manifest 和 stable**

- 测试层：RustFS 发布集成
- 状态：`部分覆盖`
- 证据：[发布失败语义](../../../repository.md#第三方仓库发布流程)
- 缺口：缺失本地资产已有直接断言，尚未注入 S3 上传中途失败。

## PUB-07

**匿名 HTTP 客户端能读取 descriptor、manifest 和全部资产**

- 测试层：RustFS 发布集成、Docker E2E
- 状态：`已覆盖`
- 证据：[发布集成脚本](../../../../scripts/test-publish-http-repository.sh)、[Docker 功能 E2E](../../docker-e2e.md)

## PUB-08

**真实 Release 发布到生产 RustFS 后，由 `lkit` 从公开地址完成安装**

- 测试层：生产 smoke
- 状态：`待补充`
- 说明：[公网 Smoke Test 边界](../../../repository.md#公网-smoke-test)
