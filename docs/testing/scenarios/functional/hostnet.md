# 宿主网络适配场景

## HNET-01

**递归收集 ifupdown 主文件、source 与 source-directory 文件**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[文件收集测试](../../../../crates/lkit-hostnet/src/ifupdown/collect.rs)、[hostnet 设计](../../../network/hostnet.md)
- 说明：覆盖相对路径、多路径段 glob、字符类、source-directory 文件名过滤、排序和 canonical 去重。

## HNET-02

**解析 Debian ifupdown 常用合法语法且保守拒绝不安全展开**

- 测试层：Rust 单元、Debian 容器
- 状态：`已覆盖`
- 证据：[解析器测试](../../../../crates/lkit-hostnet/src/ifupdown/parse.rs)、[真实 ifupdown 测试](../../../../crates/lkit-hostnet/tests/ifupdown_real.rs)
- 说明：覆盖非缩进选项、续行、CRLF 行尾、inherits、allow-*、mapping、rename、source-directory；不执行 shell 变量或命令替换。

## HNET-03

**选中接口改为 manual 并退出 ifupdown 自动选择**

- 测试层：Rust 单元、Rust 集成
- 状态：`已覆盖`
- 证据：[编辑测试](../../../../crates/lkit-hostnet/src/ifupdown/edit.rs)、[全流程测试](../../../../crates/lkit-hostnet/tests/ifupdown_flow.rs)
- 说明：删除选中 stanza 的 inherits 与全部选项，并从 auto、allow-*、no-auto-down、no-scripts 中删除选中接口；无关内容保持原样。

## HNET-04

**不安全文件类型和接口依赖在任何写入前阻断**

- 测试层：Rust 单元、Rust 集成
- 状态：`已覆盖`
- 证据：[收集安全测试](../../../../crates/lkit-hostnet/src/ifupdown/collect.rs)、[编辑 preflight](../../../../crates/lkit-hostnet/src/ifupdown/edit.rs)
- 说明：覆盖配置符号链接、PPP、mapping/rename 模式、inherits 反向依赖、bridge_ports、bond-slaves 和相对入口路径。

## HNET-05

**备份与恢复逐字保留内容及 mode、uid、gid**

- 测试层：Rust 单元、Rust 集成
- 状态：`已覆盖`
- 证据：[备份恢复测试](../../../../crates/lkit-hostnet/src/ifupdown/backup.rs)、[全流程测试](../../../../crates/lkit-hostnet/tests/ifupdown_flow.rs)
- 说明：备份文件和 manifest 使用私有权限；恢复幂等；ACL/xattr 不在当前范围。

## HNET-06

**计划生成后的内容或元数据漂移不会被覆盖，安全文件仍会回滚**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[并发修改测试](../../../../crates/lkit-hostnet/src/ifupdown/edit.rs)、[备份安全测试](../../../../crates/lkit-hostnet/src/ifupdown/backup.rs)
- 说明：外部漂移文件保持原样，同时回滚仍处于本次编辑结果的文件。

## HNET-07

**高层事务入口在应用或校验失败后自动恢复**

- 测试层：Rust 单元、Rust 集成
- 状态：`已覆盖`
- 证据：[适配器事务测试](../../../../crates/lkit-hostnet/src/adapter.rs)、[失败恢复全流程](../../../../crates/lkit-hostnet/tests/ifupdown_flow.rs)
- 说明：恢复也失败时同时保留原始错误与恢复错误；外部漂移文件保持原样。

## HNET-08

**真实 Debian ifupdown 接受改写后的临时配置**

- 测试层：Debian 容器 CI
- 状态：`已覆盖`
- 证据：[真实 ifupdown 测试](../../../../crates/lkit-hostnet/tests/ifupdown_real.rs)、[容器 workflow](../../../../.github/workflows/test-hostnet-ifupdown.yml)
- 说明：容器不挂载宿主 `/etc`，ifup 始终通过 `--interfaces=<临时文件>` 读取 fixture。

## HNET-09

**按 manifest 反查上一次摘除实际移出的接口集合**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[unmanaged_interfaces 测试](../../../../crates/lkit-hostnet/src/ifupdown/mod.rs)、
  [hostnet 设计](../../../network/hostnet.md)
- 说明：当前文件集合中 manual-bare、同时在 manifest 原始快照中非 manual-bare 的接口
  记为已摘除；宿主自声明的 manual-bare stanza、摘除后新增的 stanza 与宿主改回带配置
  的 stanza 都不算。lkit-cli 的 reinit 同接口集校验（REI-11）消费该结果。


## HNET-10

**NetworkManager 适配器写入 conf.d drop-in 并对称恢复**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[NM 适配器测试](../../../../crates/lkit-hostnet/src/nm/mod.rs)、
  [hostnet 设计](../../../network/hostnet.md)
- 说明：`unmanaged-devices=interface-name:<if>;...` 排序去重；新建 drop-in（0644、
  属主继承 conf.d）恢复时删除，既有同名文件逐字备份后改写、恢复逐字还原；glob 元字符
  选中名拒绝；guarded 恢复保留外部修改。运行时 reload 由 lkit-cli 负责（NET-14）。

## HNET-11

**firewalld 适配器删除 zone XML 中选中接口的整行并保守拒绝混合形态**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[firewalld 适配器测试](../../../../crates/lkit-hostnet/src/firewalld/mod.rs)、
  [hostnet 设计](../../../network/hostnet.md)
- 说明：仅删除自闭合单属性 `<interface name="..."/>` 整行，其余字节保留；选中接口仍以
  其他形态出现在任何 `<interface>` 元素中时计划阶段整体拒绝、不改任何文件。运行时
  reload 由 lkit-cli 负责（NET-14）。

## HNET-12

**NM 与 firewalld 适配器各自反查摘除现场**

- 测试层：Rust 单元
- 状态：`已覆盖`
- 证据：[NM 反查测试](../../../../crates/lkit-hostnet/src/nm/mod.rs)、
  [firewalld 反查测试](../../../../crates/lkit-hostnet/src/firewalld/mod.rs)
- 说明：NM 读取现场 drop-in 的 `unmanaged-devices` 条目；firewalld 取
  "manifest 快照整行接口名 − 现场仍存在名字"差集。lkit-cli 的 reinit 同接口集校验
  （REI-11）消费两者与 ifupdown 反查（HNET-09）的并集。
