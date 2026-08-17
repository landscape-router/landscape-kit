# 网络向导样式验收标准

- 来源:Install/Reinit 面板激活后的全屏页面(无侧栏)
- 对应实现:`console/network_wizard/render.rs`

## 布局

- 全屏无侧栏:步骤标题 + 内容 + 灰字底栏提示;
- 外层带边框标题 `网络`;Esc 非首页步骤返回上一步。

## 步骤标题与列表

- 步骤标题:加粗(WAN 选择 / WAN 配置 / LAN 选择 / LAN DHCP 配置 /
  计划摘要);
- WAN 列表行:`index + name + mac + 首 IPv4 + gw 网关`:
  - 选中 `FOCUS_SELECTED` 反色 + `> ` 标记;
  - 未选中默认色;无 IPv4/网关显示灰字占位;
- LAN 列表行:`[x]/[ ] + name + mac + link up/down`:
  - 光标行 `FOCUS_SELECTED` 反色 + `> ` 标记;
  - 勾选 `[x]` 与未勾选 `[ ]`;无其他网卡时灰字提示。

## WAN 配置页

- 顶部两个 tab `[ Static ]` / `[ DHCP client ]`:
  - 聚焦且激活:黑底青字加粗;仅聚焦或仅激活:加粗;否则默认色;
  - Left/Right 切换,切换保留已填静态值;
- 静态模式:IPv4 地址/CIDR 与默认网关两个可编辑字段(同 Install 表单:
  聚焦反色 + `> `、编辑中 `_` 光标、整行青色背景);
- DHCP 模式:灰字说明行;
- 底部"确认并继续"按钮:聚焦 `FOCUS_SELECTED` 反色 + `> ` 标记,
  未聚焦默认色。

## LAN DHCP 配置页

- 管理地址、DHCP 地址池起始/结束三个字段同页编辑(样式同上),
  底部"确认并继续"按钮一次性校验并进入摘要。

## 计划摘要页

- 加粗标题;WAN 接口与 MAC、Static IPv4/网关或 DHCP、LAN/WAN-only 模式、
  管理地址与 DHCP 范围、LAN 清理提示(黄字)、
  白字加粗 "按 Enter 开始安装"。

## 取消确认层

- 居中带边框标题;加粗问题、Enter 行、灰字 Esc 行;
- 弹窗内点击=Enter、弹窗外=Esc。

## 证据

- `console/network_wizard/render.rs`;
- 测试:`console/tests/wizard.rs`。
