use ratatui::layout::Rect;
use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::install_form::InstallField;
use super::network_wizard::WanMode;
use super::reinit::ReinitField;
use super::update::UpdateField;
use crate::mirror::MirrorName;
use crate::software::Software;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Menu {
    Overview,
    Install,
    Backup,
    Update,
    Mirror,
    Software,
    Reinit,
    /// 卸载面板：暂时从 TUI 侧栏隐藏（功能经 `lkit uninstall` CLI 提供），
    /// 面板渲染、键处理与确认层代码保留；重新启用时把 `Self::Uninstall`
    /// 加回 `ALL` 并放开 `menu_available` 的注释。
    #[allow(dead_code)]
    Uninstall,
}

impl Menu {
    pub(crate) const ALL: [Self; 7] = [
        Self::Overview,
        Self::Install,
        Self::Backup,
        Self::Update,
        Self::Mirror,
        Self::Software,
        Self::Reinit,
        // Self::Uninstall, // TODO(uninstall-console): 暂隐藏,CLI `lkit uninstall` 保留
    ];

    pub(crate) fn label(self) -> String {
        match self {
            Self::Overview => crate::tr!(crate::keys::CONSOLE_OVERVIEW),
            Self::Install => crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
            Self::Backup => crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
            Self::Update => crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
            Self::Mirror => crate::tr!(crate::keys::CONSOLE_MIRROR_MENU),
            Self::Software => crate::tr!(crate::keys::CONSOLE_SOFTWARE_MENU),
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
    /// 安装面板表单行。
    InstallField(InstallField),
    /// 更新面板表单行。
    UpdateField(UpdateField),
    /// 换源面板镜像行。
    MirrorField(MirrorName),
    /// 换源面板“恢复备份”动作行。
    MirrorRestore,
    /// 换源确认层：security 替换开关行。
    MirrorSecurityToggle,
    /// 换源确认层：CD-ROM 源注释开关行。
    MirrorCdromToggle,
    /// 软件面板软件行。
    SoftwareField(Software),
    /// 软件面板基础包行。
    SoftwareBasePackages,
    /// 基础包弹框:包行(切换勾选)。
    BasePackageRow(usize),
    /// 基础包弹框:确认动作行(视为 Enter)。
    BasePackageConfirm,
    /// 软件确认层：来源切换行。
    SoftwareSourceToggle,
    /// 卸载面板“执行卸载”动作行。
    UninstallAction,
    /// Overview 面板“部署 daemon”动作行(视为 Enter)。
    OverviewDeploy,
    /// Overview 面板“查看急救恢复码”动作行(视为 Enter)。
    OverviewShowPsk,
    /// 安装阻断弹框内“部署 daemon”按钮(视为按 D)。
    DeployDaemon,
    /// reinit 面板:可编辑凭据行。
    ReinitField(ReinitField),
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
    /// 底栏语言指示:等价于按 L,切换到所示目标语言。
    LanguageSwitch,
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
pub(crate) fn wrapped_rows(width: u16, text: &str) -> u16 {
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

/// 按显示宽度预折行文本(优先在空格断行,超宽的无空格段按字符硬切),每行实际
/// 宽度不超过 `width`;输入中的 `\n` 先分段再逐段折行,分段边界总是保留为行边界。
/// 折行后的行交给 Paragraph 渲染不会再触发其词级换行,`block_row_of` 的按字符
/// 换行模拟才能与实际渲染行号一致(词级换行会预留行尾空白,两级行号会漂移,
/// 命中区会错位);底栏高度计算同理复用同一折行结果,消除"模拟按字符、渲染按
/// 单词"的高度偏差。
pub(crate) fn wrap_to_width(width: u16, text: &str) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split(' ') {
        // 换行符不可显示且必须作为硬行边界,先于此处分段,避免被当作普通字符
        // 留在行内破坏行数统计。
        for (index, segment) in word.split('\n').enumerate() {
            if index > 0 {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            wrap_segment(width, segment, &mut lines, &mut current, &mut current_width);
        }
    }
    lines.push(current);
    lines
}

fn wrap_segment(
    width: usize,
    segment: &str,
    lines: &mut Vec<String>,
    current: &mut String,
    current_width: &mut usize,
) {
    let segment_width = UnicodeWidthStr::width(segment);
    if *current_width > 0 && *current_width + 1 + segment_width <= width {
        current.push(' ');
        current.push_str(segment);
        *current_width += 1 + segment_width;
        return;
    }
    if *current_width > 0 {
        lines.push(std::mem::take(current));
        *current_width = 0;
    }
    for character in segment.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if *current_width > 0 && *current_width + character_width > width {
            lines.push(std::mem::take(current));
            *current_width = 0;
        }
        current.push(character);
        *current_width += character_width;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_to_width_breaks_english_on_word_boundaries() {
        let lines = wrap_to_width(12, "deploy the resident service");
        assert_eq!(lines, vec!["deploy the", "resident", "service"]);
        assert!(lines.iter().all(|line| line.width() <= 12));
    }

    #[test]
    fn wrap_to_width_hard_splits_unspaced_cjk_text() {
        let lines = wrap_to_width(8, "以 systemd 服务常驻后台");
        assert_eq!(
            lines,
            vec!["以", "systemd", "服务常驻", "后台"],
            "a word that cannot share a line is hard-split at the width boundary"
        );
        assert!(lines.iter().all(|line| line.width() <= 8));
        assert_eq!(
            lines.concat().replace(' ', ""),
            "以systemd服务常驻后台",
            "hard splitting must not drop characters"
        );
    }

    #[test]
    fn wrapped_rows_counts_each_prewrapped_line_as_one_row() {
        let text = "the resident service executes privileged operations";
        for line in wrap_to_width(16, text) {
            assert_eq!(wrapped_rows(16, &line), 1);
        }
    }

    #[test]
    fn wrap_to_width_keeps_newlines_as_hard_line_boundaries() {
        let lines = wrap_to_width(20, "applied\nrefreshing the package index");
        assert_eq!(lines, vec!["applied", "refreshing the", "package index"]);
        assert!(lines.iter().all(|line| line.width() <= 20));
    }

    #[test]
    fn wrap_to_width_resumes_the_same_line_after_a_newline() {
        // 换行分段后,新段落从空行开始,可与后续单词合并,不会残留上一段的宽度。
        let lines = wrap_to_width(12, "done\nnext words");
        assert_eq!(lines, vec!["done", "next words"]);
    }
}
