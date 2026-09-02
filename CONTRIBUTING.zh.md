# 贡献指南

感谢你为 Landscape Kit 贡献代码。本项目支持 AI 辅助（"vibe code"）开发：直接用 AI
扫描项目、创建 issue、编写代码并提交 PR 即可。项目采用 **issue 驱动的工作流**。

## 工作流

1. **先扫描项目。** 在提出任何建议前，先阅读 `README.md`、`docs/README.md` 和
   `AGENTS.md`。`docs/` 中的规格定义了预期行为：变更应基于这些规格提出，行为变化时
   也要同步更新文档。

2. **检查重复。** 在创建 issue 前先搜索现有 issue（使用 `gh issue list --search ...`
   或 GitHub 搜索框）。如果已有相关 issue，请加入讨论而不是新开一个。

3. **按模板创建 issue。** 选择与你的请求匹配的模板（[缺陷报告](.github/ISSUE_TEMPLATE/bug-report.zh.yml)
   或 [功能请求](.github/ISSUE_TEMPLATE/feature-request.zh.yml)，英文版见
   [bug-report.en.yml](.github/ISSUE_TEMPLATE/bug-report.en.yml) 和
   [feature-request.en.yml](.github/ISSUE_TEMPLATE/feature-request.en.yml)），填写所有字段。
   一个完整的 issue 需要描述受影响命令、当前行为、期望行为和验收标准。

4. **实现。** 遵循 `AGENTS.md` 中的约定：提交前运行 `cargo fmt`，为你的变更编写单元
   测试（例如 `cargo test -p lkit-cli <module-filter>`），行为变化时更新 `docs/`。

5. **发起 pull request。** 在 PR 描述中关联 issue（例如 `Closes #123`）并填写 PR 模板
   中的勾选清单。

## 测试辅助二进制

测试套件不会接触真实的 systemd 或真实的 `landscape-webserver`，而是由
`lkit-test-fixture` crate（`crates/lkit-test-fixture/`）提供的假程序顶替，它们的行为
由 JSON 配置文件驱动：

- `lkit-test-systemctl`——假的 `systemctl`（兼 `systemd-analyze`），unit 文件、服务
  状态与命令结果都来自 `LKIT_TEST_SYSTEMCTL_CONFIG` 环境变量指向的 JSON 文件。单元
  测试通过该配置把被测代码指到它；e2e fixture 套件则直接调用它来预置既有服务。
- `lkit-test-init`——同样方式使用的假 SysV `init`（`LKIT_TEST_INIT_CONFIG`）。
- `lkit-landscape-fixture`——在 e2e fixture 套件里顶替下载的 `landscape-webserver`
  资产，按剧本提供发布内容，让 `lkit self update`/安装流程面对一个假上游运行。

通常不需要手动构建它们：e2e 套件通过 `env!("CARGO_BIN_EXE_<name>")` 引用，
`cargo test` 会自动构建；这些二进制在 `lkit-cli/Cargo.toml` 中以 `test-support`
feature 声明（fixture crate 内部还自带 `landscape-webserver`）。新增 fixture 程序时，
记得在 `lkit-cli/Cargo.toml` 注册带 `required-features = ["test-support"]` 的
`[[bin]]` 条目，否则 `CARGO_BIN_EXE_<name>` 无法解析。

## 问题咨询

不属于 issue 的问题请使用 GitHub Discussions，或直接向相关仓库的 issue 追踪器提问。
