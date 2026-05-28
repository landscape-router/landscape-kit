# AGENTS.md

Landscape 本机 CLI 管理与救援工具。独立仓库，二进制入口 `lkit`，AGPL-3.0。
首版运行在 Landscape 所在主机，解决本机安装、管理、离线救援。不做守护进程，不新增外部 API。

```
landscape-kit/
  crates/
    lkit-core/          # 公共模型、配置、错误
    lkit-client/        # Landscape API 客户端
    lkit-app/           # 用例层（业务逻辑）
    lkit-cli/           # CLI 入口 + 引导式交互
  docs/
    spec/               # 设计规格
    CONVENTIONS.md      # Rust 编码约定
  .github/              # PR / Issue 模板
  CONTRIBUTING.md       # 贡献流程
```

## Rust 编码约束

- 分层不可反向：lkit-cli → lkit-app → lkit-client → lkit-core
- lkit-app 不依赖 clap/dialoguer/console/indicatif；lkit-cli 不写业务逻辑
- core/client/app 用 thiserror；cli 用 anyhow
- 禁止 unwrap() / expect() / unimplemented!()
- 库 crate 禁止调用 `#[tokio::main]` 或直接实例化 runtime；tokio runtime 仅在 `lkit-cli` 的 `main()` 启动，main() 负责依赖注入组装
- 单元测试放 #[cfg(test)] mod tests
- 禁止 unsafe、禁止硬编码凭据
- 涉及修改或审查 Rust 代码时，MUST 先读取 `docs/CONVENTIONS.md` 并逐条遵守

## 贡献流程

- PR 和 Issue MUST 遵守对应模板（`.github/PULL_REQUEST_TEMPLATE.md`、`.github/ISSUE_TEMPLATE/`）

## 开发命令

Workspace 尚未初始化。初始化后补全：`cargo build`、`cargo test --workspace`、`cargo fmt --all -- --check`、`cargo clippy --all -- -D warnings`

## 效率

- 不要在编辑中途跑 `cargo fmt`，最后跑一次即可。`cargo check` 随时可用，禁止 `cargo fix`
- 长输出用 `| tail -30` 截断摘要，不要全量灌入上下文
- 开发结束一次性验证：`cargo fmt --all && cargo clippy --all -- -D warnings && cargo test`

## 收尾

代码变更涉及行为、接口、架构调整时，MUST 同步更新 `docs/spec/` 中对应文档。文档与代码不一致视为未完成。