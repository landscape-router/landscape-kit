use ratatui::layout::Rect;
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use super::network_wizard::WanMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Menu {
    Overview,
    Install,
    Backup,
    Update,
    Reinit,
    /// 卸载面板：暂时从 TUI 侧栏隐藏（功能经 `lkit uninstall` CLI 提供），
    /// 面板渲染、键处理与确认层代码保留；重新启用时把 `Self::Uninstall`
    /// 加回 `ALL` 并放开 `menu_available` 的注释。
    #[allow(dead_code)]
    Uninstall,
}

impl Menu {
    pub(crate) const ALL: [Self; 5] = [
        Self::Overview,
        Self::Install,
        Self::Backup,
        Self::Update,
        Self::Reinit,
        // Self::Uninstall, // TODO(uninstall-console): 暂隐藏,CLI `lkit uninstall` 保留
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Overview => crate::tr!(crate::keys::CONSOLE_OVERVIEW),
            Self::Install => crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
            Self::Backup => crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
            Self::Update => crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
            Self::Reinit => crate::tr!(crate::keys::CONSOLE_REINIT_MENU),
            Self::Uninstall => crate::tr!(crate::keys::CONSOLE_UNINSTALL_MENU),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Focus {
    Navigation,
    Panel,
}

/// 鼠标左键可命中的界面元素。命中后按对应键盘语义处理(如 Enter/Esc),
/// 保证鼠标与键盘行为一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Hit {
    /// 点击弹层内部但不触发任何动作(如输入框内点击)。
    Nothing,
    /// 弹层外的整屏区域:视为 Esc。
    Outside,
    /// 确认类弹层整体:视为 Enter。
    DialogConfirm,
    /// 侧栏区域:聚焦导航。
    Navigation,
    /// 面板区域:聚焦面板。
    Panel,
    /// 侧栏菜单项。
    Menu(usize),
    /// 安装面板“环境检查”行。
    InstallChecks,
    /// 安装面板表单行(索引与 `InstallForm.selected` 一致)。
    InstallField(usize),
    /// 更新面板表单行。
    UpdateField(usize),
    /// 卸载面板“执行卸载”动作行。
    UninstallAction,
    /// reinit 面板:可编辑凭据行(0=admin 用户,1=密码)。
    ReinitField(usize),
    /// reinit 面板:开始/执行动作行(视为 Enter)。
    ReinitAction,
    /// 备份面板行:0 为“创建备份”,其余为备份条目。
    BackupRow(usize),
    /// 网络向导:WAN 接口行。
    WizardWan(usize),
    /// 网络向导:WAN 模式页 Tab。
    WizardTab(WanMode),
    /// 网络向导:可编辑字段行(页面内焦点序号)。
    WizardField(usize),
    /// 网络向导:LAN 候选行(切换勾选)。
    WizardLan(usize),
    /// 网络向导:继续/确认(视为 Enter)。
    WizardContinue,
    /// 阻塞接管屏:选择行(0=稍后,1=确认接管)。
    TakeoverChoice(usize),
}

/// 渲染时收集的可点击区域;后注册者优先(弹层覆盖底层界面)。
#[derive(Default)]
pub(crate) struct Clicks {
    regions: Vec<(Rect, Hit)>,
}

impl Clicks {
    pub(crate) fn clear(&mut self) {
        self.regions.clear();
    }

    pub(crate) fn add(&mut self, area: Rect, hit: Hit) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.regions.push((area, hit));
    }

    /// 注册带边框块内第 `row` 行(0-based 内容行,不含边框)的可点击区。
    pub(crate) fn block_row(&mut self, area: Rect, row: u16, hit: Hit) {
        self.add(
            Rect::new(
                area.x.saturating_add(1),
                area.y.saturating_add(1).saturating_add(row),
                area.width.saturating_sub(2),
                1,
            ),
            hit,
        );
    }

    /// 命中测试:从后向前(后绘制者优先),返回命中区域对应的动作。
    pub(crate) fn hit_at(&self, column: u16, row: u16) -> Option<Hit> {
        self.regions
            .iter()
            .rev()
            .find(|(area, _)| {
                column >= area.x
                    && column < area.x.saturating_add(area.width)
                    && row >= area.y
                    && row < area.y.saturating_add(area.height)
            })
            .map(|(_, hit)| *hit)
    }
}

/// 模拟 Paragraph 在指定宽度下的按字符换行,返回占用的行数。
fn wrapped_rows(width: u16, text: &str) -> u16 {
    let width = usize::from(width.max(1));
    let mut rows = 1u16;
    let mut used = 0usize;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            rows = rows.saturating_add(1);
            used = character_width;
        } else {
            used = usize::min(used + character_width, width);
        }
    }
    rows
}

/// 拼接 `Line` 的全部 Span 文本,用于按宽度的换行模拟。
fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// 计算 `lines` 中第 `target` 行在带边框块内容区内的行偏移(模拟换行)。
pub(crate) fn block_row_of(lines: &[Line], target: usize, width: u16) -> u16 {
    let mut row = 0u16;
    for (index, line) in lines.iter().enumerate() {
        if index == target {
            return row;
        }
        row = row.saturating_add(wrapped_rows(width, &line_text(line)));
    }
    row
}
