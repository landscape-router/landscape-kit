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

## 问题咨询

不属于 issue 的问题请使用 GitHub Discussions，或直接向相关仓库的 issue 追踪器提问。
