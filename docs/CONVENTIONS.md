# 编码约定

所有规则为硬约束，面向 AI Agent 和贡献者。MUST / MUST NOT 不可违背。

## 1. 分层边界

- `lkit-cli` → `lkit-app` → `lkit-client` → `lkit-core`，MUST NOT 反向依赖
- `lkit-app` MUST NOT 依赖 `clap` / `dialoguer` / `console` / `indicatif`
- `lkit-cli` MUST NOT 包含业务逻辑
- 库 crate 之间通过 trait 解耦，consumer 定义 trait，producer 实现

## 2. 错误处理

| 层 | 策略 |
|---|---|
| `core` / `client` / `app` | `thiserror` 枚举，MUST NOT 使用 `anyhow` |
| `cli` | `anyhow::Result`，MUST 将库错误转为 `Error / Caused by / Suggestion` 格式 |

示例：
```
Error: 无法连接 Landscape API
Caused by: 连接超时 (127.0.0.1:8080)
Suggestion: 检查 Landscape 服务是否运行：systemctl status landscape
```

MUST NOT 使用 `unwrap()` / `expect()`（含测试代码）。MUST NOT 使用 `unimplemented!()`。MUST NOT 吞掉错误（`let _ = fallible()` 需注释说明原因）。

## 3. 模块组织

- `lkit-app` 按用例切模块：`install` / `backup` / `upgrade` / `status` / `diagnose` / `config` / `self_upgrade`
- 每个用例：struct 暴露，构造注入依赖，方法返回 `Result<T, AppError>`
- `pub(crate)` 用于 crate 内部共享，`pub` 仅用于 crate 公共 API

## 4. 依赖注入

- client trait 定义在 `lkit-core`（消费者定义接口），实现在 `lkit-cli` 或 `lkit-client`
- 实现在 `lkit-cli` 的 `main()` 中组装注入
- 测试时用 mock 实现 trait

## 5. 测试

- 单元测试放同文件底部 `#[cfg(test)] mod tests`
- 集成测试放 crate 的 `tests/` 目录
- 测试函数 MUST 有断言，禁止只有 `println!()` 的测试
- MUST NOT 依赖外部网络或 Landscape 进程（使用 trait mock 替代，见第 4 节）

## 6. 异步

- tokio runtime MUST 仅在 `lkit-cli` 的 `main()` 创建
- 库函数标记 `async`，MUST NOT 自行创建 runtime
- MUST NOT 使用 `std::thread::spawn`

## 7. 命名与格式

- MUST 通过 `cargo fmt --all -- --check`
- MUST 通过 `cargo clippy --all -- -D warnings`
- `use` 导入顺序：`std` → 第三方 → crate 内，组间空行
- 类型 PascalCase，函数/变量 snake_case，常量 SCREAMING_SNAKE_CASE

## 8. 文档注释

- `pub` 类型和 `pub` 函数 MUST 有 `///` 文档注释
- 模块级注释用 `//!` 放在文件顶部
- 注释解释 WHY（非直观行为、性能考虑、已知限制），不解释 WHAT

## 9. 安全与健壮

- MUST NOT 使用 `unsafe` 代码块
- 路径操作 MUST 使用规范化和逃逸检查
- 外部命令执行 MUST 使用参数列表而非拼接 shell 字符串
- MUST NOT 硬编码密钥/凭据
