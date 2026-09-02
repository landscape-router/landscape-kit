use ratatui::style::Color;

/// 底栏状态消息。级别决定底栏染色:Ready 灰、Info 黄、Success 绿、Error 红;
/// 取代旧约定里以 `"Ready"` 哨兵字符串表示"无消息"、其余一律红字的写法。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum Notice {
    /// 就绪,无消息。文案在渲染时按当前语言动态翻译(语言可随时切换)。
    #[default]
    Ready,
    /// 过程信息与温和提示:后台任务进行中、等待,或引导用户先选择目标。
    Info(String),
    /// 操作成功的结果。
    Success(String),
    /// 失败与阻断。
    Error(String),
}

impl Notice {
    /// 底栏显示文本。
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Ready => crate::tr!(crate::keys::CONSOLE_READY),
            Self::Info(text) | Self::Success(text) | Self::Error(text) => text.clone(),
        }
    }

    pub(crate) fn color(&self) -> Color {
        match self {
            Self::Ready => Color::DarkGray,
            Self::Info(_) => Color::Yellow,
            Self::Success(_) => Color::Green,
            Self::Error(_) => Color::Red,
        }
    }

    /// 追加一行(换源/恢复的多行结果在其上继续拼接刷新状态);Ready 时升级为
    /// Info,已有消息保持原级别与染色。
    pub(crate) fn push_line(&mut self, line: String) {
        match self {
            Self::Ready => *self = Self::Info(line),
            Self::Info(text) | Self::Success(text) | Self::Error(text) => {
                text.push('\n');
                text.push_str(&line);
            }
        }
    }
}

#[cfg(test)]
impl Notice {
    /// 供测试断言消息内容;Ready 视为无消息。
    pub(crate) fn contains(&self, needle: &str) -> bool {
        match self {
            Self::Ready => false,
            Self::Info(text) | Self::Success(text) | Self::Error(text) => text.contains(needle),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_text_is_translated_lazily() {
        let previous = crate::i18n::current();
        crate::i18n::configure(crate::i18n::Language::Zh);
        assert_eq!(Notice::Ready.text(), "就绪");
        crate::i18n::configure(previous);
    }

    #[test]
    fn push_line_appends_to_messages_and_upgrades_ready() {
        let mut notice = Notice::Success("applied".into());
        notice.push_line("refreshing".into());
        assert_eq!(notice.text(), "applied\nrefreshing");
        assert_eq!(notice.color(), Color::Green, "the base level must be kept");

        let mut notice = Notice::Ready;
        notice.push_line("refreshing".into());
        assert_eq!(notice.text(), "refreshing");
        assert_eq!(notice.color(), Color::Yellow);
    }

    #[test]
    fn levels_map_to_footer_colors() {
        assert_eq!(Notice::Ready.color(), Color::DarkGray);
        assert_eq!(Notice::Info(String::new()).color(), Color::Yellow);
        assert_eq!(Notice::Success(String::new()).color(), Color::Green);
        assert_eq!(Notice::Error(String::new()).color(), Color::Red);
    }
}
