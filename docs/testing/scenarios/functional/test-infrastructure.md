# 测试执行可靠性

## TST-01

**测试过滤器必须匹配真实用例，并输出执行清单**

- 测试层：Python 隔离单元、Rust 与 fixture CI
- 状态：`已覆盖`
- 证据：[清单校验器](../../../../scripts/ci/run-test-selection.py)、[校验器测试](../../../../scripts/ci/test_test_selection.py)、[执行契约](../../reliability.md)
- 说明：拒绝空清单、重复过滤器和零匹配；未选测试单独列出，不冒充全量覆盖。

## TST-02

**失败现场可归档，归档不阻止 fixture 清理**

- 测试层：临时目录单元、CI fixture
- 状态：`部分覆盖`
- 证据：[脱敏与边界测试](../../../../lkit-cli/src/fixture_diagnostics_tests.rs)、[panic 与清理场景](../../../../lkit-cli/tests/install_fixture_e2e/fault_matrix.rs)、[fixture CI](../../../../.github/workflows/test-fixture-e2e.yml)
- 缺口：runner 强制终止时无法保证完成归档；收尾扫描是尽力执行。
- 说明：验证敏感字段不导出、日志敏感行替换、不跟随符号链接、损坏／超大文件处理以及 panic 后归档与目录清理。

## TST-03

**安装与切换的错误返回和进程崩溃分别验证恢复不变量**

- 测试层：CI fixture
- 状态：`部分覆盖`
- 证据：[故障矩阵](../../reliability.md#事务故障矩阵)、[错误注入](../../../../lkit-cli/tests/install_fixture_e2e/fault_matrix.rs)、[安装强杀](../../../../lkit-cli/tests/install_fixture_e2e/install.rs)、[切换强杀](../../../../lkit-cli/tests/install_fixture_e2e/switch.rs)
- 缺口：真实磁盘故障、断电及其余操作／后端交叉矩阵。
- 说明：注入错误必须命中指定检查点，验证回滚结果及再次重启不重复执行。
