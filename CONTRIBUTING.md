# 贡献指南

## 开发流程

Fork → 分支 → 开发 → PR。PR MUST 使用 PR 模板，所有 checklist 项必须通过。

PR 合并通过后，当前仓库采用以下合并策略。

- Squash and merge：默认策略，适用于大部分 PR。内部迭代的 commit 不保留，主线干净
- Rebase and merge：每个 commit 独立可读、有独立价值时使用，保留线性历史
- Merge commit：跨多模块的大 PR，commit 分组有阅读价值时使用

## Issue 规范

提交 Issue MUST 使用对应的 Issue 模板（Bug 报告 / 功能请求），选择正确的标签。

## 编码约定

所有代码变更 MUST 遵守 `docs/CONVENTIONS.md`。

## 许可证

AGPL-3.0。贡献即表示同意以相同许可证授权。
