use std::cell::Cell;
use std::ffi::OsString;
use std::sync::atomic::{AtomicU8, Ordering};

use clap::error::{ContextKind, ContextValue, ErrorKind};

pub(crate) const LANGUAGE_ENV: &str = "LKIT_LANG";

const EN: u8 = 0;
const ZH: u8 = 1;
static LANGUAGE: AtomicU8 = AtomicU8::new(EN);
thread_local! {
    static THREAD_LANGUAGE: Cell<Option<Language>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Language {
    En,
    Zh,
}

impl Language {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }

    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::En => Self::Zh,
            Self::Zh => Self::En,
        }
    }

    fn from_override(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "zh" => Self::Zh,
            _ => Self::En,
        }
    }

    fn from_system_locale(value: &str) -> Self {
        let primary = value
            .trim()
            .split(|character| matches!(character, '_' | '-' | '.' | '@'))
            .next()
            .unwrap_or_default();
        match primary.to_ascii_lowercase().as_str() {
            "zh" => Self::Zh,
            _ => Self::En,
        }
    }
}

pub(crate) fn configure(language: Language) {
    #[cfg(not(test))]
    LANGUAGE.store(
        match language {
            Language::En => EN,
            Language::Zh => ZH,
        },
        Ordering::SeqCst,
    );
    THREAD_LANGUAGE.set(Some(language));
}

pub(crate) fn current() -> Language {
    THREAD_LANGUAGE
        .get()
        .unwrap_or_else(|| match LANGUAGE.load(Ordering::SeqCst) {
            ZH => Language::Zh,
            _ => Language::En,
        })
}

pub(crate) fn with_language<T>(language: Language, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<Language>);

    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD_LANGUAGE.set(self.0);
        }
    }

    let previous = THREAD_LANGUAGE.replace(Some(language));
    let _restore = Restore(previous);
    operation()
}

pub(crate) fn select<'a>(english: &'a str, chinese: &'a str) -> &'a str {
    match current() {
        Language::En => english,
        Language::Zh => chinese,
    }
}

pub(crate) fn resolve(explicit: Option<&str>) -> Language {
    if let Some(value) = explicit {
        return Language::from_override(value);
    }
    if let Some(value) = nonempty_environment(LANGUAGE_ENV) {
        return value
            .to_str()
            .map(Language::from_override)
            .unwrap_or(Language::En);
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(nonempty_environment)
        .and_then(|value| value.to_str().map(Language::from_system_locale))
        .unwrap_or(Language::En)
}

/// Select the language before Clap renders help or a parse error.
pub(crate) fn preconfigure(args: impl IntoIterator<Item = OsString>) {
    let args: Vec<OsString> = args.into_iter().collect();
    configure(explicit_language(&args).unwrap_or_else(|| resolve(None)));
}

fn explicit_language(args: &[OsString]) -> Option<Language> {
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        if argument == "--lang" {
            return Some(
                args.next()
                    .and_then(|value| value.to_str())
                    .map(Language::from_override)
                    .unwrap_or(Language::En),
            );
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--lang="))
        {
            return Some(Language::from_override(value));
        }
    }
    None
}

fn nonempty_environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

pub(crate) fn print_clap_error(error: &clap::Error) {
    if current() == Language::En
        || matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | ErrorKind::DisplayVersion
                | ErrorKind::Io
                | ErrorKind::Format
        )
    {
        let _ = error.print();
        return;
    }

    eprintln!("错误：{}", chinese_clap_message(error));
    print_clap_suggestion(error, ContextKind::SuggestedSubcommand, "子命令");
    print_clap_suggestion(error, ContextKind::SuggestedArg, "参数");
    print_clap_suggestion(error, ContextKind::SuggestedValue, "值");
    if let Some(usage) = error.get(ContextKind::Usage) {
        let usage = usage.to_string();
        let usage = usage.strip_prefix("Usage: ").unwrap_or(&usage);
        eprintln!("\n用法：{usage}");
    }
    eprintln!("\n更多信息请尝试 '--help'。");
}

fn chinese_clap_message(error: &clap::Error) -> String {
    let invalid_arg = clap_context(error, ContextKind::InvalidArg);
    let invalid_subcommand = clap_context(error, ContextKind::InvalidSubcommand);
    let invalid_value = clap_context(error, ContextKind::InvalidValue);
    match error.kind() {
        ErrorKind::ArgumentConflict => match (invalid_arg, invalid_subcommand) {
            (Some(argument), _) => format!("参数 '{argument}' 与其他已指定参数冲突"),
            (_, Some(subcommand)) => format!("子命令 '{subcommand}' 与其他已指定参数冲突"),
            _ => "指定了不能同时使用的参数".into(),
        },
        ErrorKind::NoEquals => match invalid_arg {
            Some(argument) => format!("为参数 '{argument}' 赋值时必须使用等号"),
            None => "为参数赋值时必须使用等号".into(),
        },
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
            match (invalid_arg, invalid_value) {
                (Some(argument), Some(value)) if value.is_empty() => {
                    format!("参数 '{argument}' 需要一个值")
                }
                (Some(argument), Some(value)) => {
                    let mut message = format!("参数 '{argument}' 的值 '{value}' 无效");
                    if let Some(valid) = clap_context(error, ContextKind::ValidValue) {
                        message.push_str(&format!("\n\n可选值：{valid}"));
                    }
                    message
                }
                _ => "参数值无效".into(),
            }
        }
        ErrorKind::InvalidSubcommand => match invalid_subcommand {
            Some(subcommand) => format!("无法识别子命令 '{subcommand}'"),
            None => "无法识别子命令".into(),
        },
        ErrorKind::MissingRequiredArgument => match invalid_arg {
            Some(arguments) => format!("缺少必需参数：{arguments}"),
            None => "缺少必需参数".into(),
        },
        ErrorKind::MissingSubcommand => match invalid_subcommand {
            Some(command) => format!("'{command}' 需要指定子命令"),
            None => "需要指定子命令".into(),
        },
        ErrorKind::InvalidUtf8 => "参数包含无效的 UTF-8 文本".into(),
        ErrorKind::TooManyValues => match (invalid_arg, invalid_value) {
            (Some(argument), Some(value)) => {
                format!("参数 '{argument}' 不接受多余的值 '{value}'")
            }
            _ => "参数值过多".into(),
        },
        ErrorKind::TooFewValues => match invalid_arg {
            Some(argument) => format!("参数 '{argument}' 需要更多值"),
            None => "参数值不足".into(),
        },
        ErrorKind::WrongNumberOfValues => match invalid_arg {
            Some(argument) => format!("参数 '{argument}' 的值数量不正确"),
            None => "参数值数量不正确".into(),
        },
        ErrorKind::UnknownArgument => match invalid_arg {
            Some(argument) => format!("无法识别参数 '{argument}'"),
            None => "无法识别参数".into(),
        },
        _ => "命令行参数无效".into(),
    }
}

fn clap_context(error: &clap::Error, kind: ContextKind) -> Option<String> {
    error.get(kind).and_then(|value| match value {
        ContextValue::None => None,
        _ => Some(value.to_string()),
    })
}

fn print_clap_suggestion(error: &clap::Error, kind: ContextKind, label: &str) {
    if let Some(value) = clap_context(error, kind) {
        eprintln!("\n提示：可能想输入{label} '{value}'。");
    }
}

#[macro_export]
macro_rules! tr {
    ($english:expr, $chinese:expr $(,)?) => {
        $crate::i18n::select($english, $chinese)
    };
}

#[macro_export]
macro_rules! trf {
    (($($english:tt)*) , ($($chinese:tt)*)) => {
        match $crate::i18n::current() {
            $crate::i18n::Language::En => format!($($english)*),
            $crate::i18n::Language::Zh => format!($($chinese)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_override_codes_and_falls_back_to_english() {
        assert_eq!(Language::from_override("en"), Language::En);
        assert_eq!(Language::from_override("zh"), Language::Zh);
        assert_eq!(Language::from_override("fr"), Language::En);
        assert_eq!(Language::from_override("zh-CN"), Language::En);
    }

    #[test]
    fn extracts_the_primary_language_from_system_locales() {
        assert_eq!(Language::from_system_locale("zh_CN.UTF-8"), Language::Zh);
        assert_eq!(Language::from_system_locale("zh-CN"), Language::Zh);
        assert_eq!(Language::from_system_locale("en_US.UTF-8"), Language::En);
        assert_eq!(Language::from_system_locale("fr_FR.UTF-8"), Language::En);
    }

    #[test]
    fn toggles_between_supported_languages() {
        assert_eq!(Language::En.toggled(), Language::Zh);
        assert_eq!(Language::Zh.toggled(), Language::En);
    }
}
