# 自定义前端

## FE-01

**自定义前端源配置解析与阻断**

- 测试层：单元
- 状态：`待补充`
- 证据：[配置文件](../deployment/config.md)、[前端开发规范](../../frontend/developer.md)
- 说明：`[frontend] active` 与 `[[frontend.sources]]` 解析；active 指向不存在的 id
  或 sources 中 id 重复/位置非法时阻断并列出合法 id；缺失 `[frontend]` 段或
  `active = "official"` 等价官方前端。

## FE-02

**GitHub 前端源 latest 解析**

- 测试层：单元 + e2e fixture
- 状态：`待补充`
- 证据：[前端开发规范](../../frontend/developer.md)
- 说明：解析 `/releases/latest`，要求 `static.zip` + `SHASUM256sum.txt`，按清单
  校验大小与 SHA-256；draft/prerelease 或资产缺失时阻断。

## FE-03

**HTTP 前端源 stable 解析**

- 测试层：单元 + e2e fixture
- 状态：`待补充`
- 证据：[前端开发规范](../../frontend/developer.md)
- 说明：`repository.json` → `channels/stable.json` → `releases/<version>/manifest.json`
  （`webserver` 空对象、只声明 `static`）；按 `assets.static` 校验下载。

## FE-04

**自定义前端应用与版本构建**

- 测试层：单元 + e2e fixture
- 状态：`待补充`
- 证据：[前端开发规范](../../frontend/developer.md)
- 说明：install/update/switch 构建版本目录后按激活源应用自定义前端，原子替换
  `releases/<version>/static/`，`static.zip` 保持官方基线；源不可达时整个命令阻断
  并提示逃生路径（移除 `[frontend]` 或 `select official`）。

## FE-05

**备份现场打包与恢复不校验**

- 测试层：单元 + e2e fixture
- 状态：`待补充`
- 证据：[备份与回滚](../../backup/lkb-and-rollback.md)
- 说明：备份从 `current/static/` 现场打包 `static.zip`（自校验）；目录含符号链接等
  非法条目时备份失败并指明条目；恢复不校验 static 身份，恢复内容即备份快照。

## FE-06

**repair static 意图驱动与 --official**

- 测试层：单元 + e2e fixture
- 状态：`待补充`
- 证据：[`lkit repair`](../../commands/repair.md)
- 说明：激活源非官方时 `repair static` 重新拉取自定义前端；否则恢复官方页面并
  更新 state 身份、刷新版本目录 `static.zip`；`--official` 无条件恢复官方并提示
  下次 switch/update 会重新应用自定义；源不可达时交互询问回退官方。
