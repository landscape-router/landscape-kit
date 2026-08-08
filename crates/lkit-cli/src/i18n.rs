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
        Self::from_code(value).unwrap_or(Self::En)
    }

    /// 严格识别支持的语言代码,不支持时返回 `None`。与 `from_override` 不同,
    /// 调用方可以把 `None` 当作"该来源不决定语言",而不是强制英文。
    pub(crate) fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "zh" => Some(Self::Zh),
            _ => None,
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

#[macro_export]
macro_rules! tr {
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        rust_i18n::t!(
            $key,
            locale = $crate::i18n::current().code(),
            $($name = $value),*
        )
        .into_owned()
    };
}

/// clap 的 help/about 需要 `&'static str`。`localized_command()` 每次进程只构建
/// 一次，这里把查询结果泄漏为静态字符串，进程生命周期内固定大小、可接受。
#[macro_export]
macro_rules! tr_static {
    ($key:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::static_str($crate::tr!($key $(, $name = $value)*))
    };
}

pub(crate) fn static_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

pub(crate) fn resolve(explicit: Option<&str>) -> Language {
    resolve_with(explicit, None)
}

/// 语言解析优先级:`--lang` > `LKIT_LANG` > 配置预设(`config.toml` 的
/// `[ui] language`) > 系统 locale > 默认英文。`configured` 只由调用方在
/// 命令行解析完成后提供,clap 帮助与解析错误阶段无法使用配置预设。
pub(crate) fn resolve_with(explicit: Option<&str>, configured: Option<Language>) -> Language {
    resolve_precedence(
        explicit,
        nonempty_environment(LANGUAGE_ENV).and_then(|value| value.into_string().ok()),
        configured,
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(nonempty_environment)
            .and_then(|value| value.into_string().ok())
            .as_deref(),
    )
}

fn resolve_precedence(
    explicit: Option<&str>,
    environment: Option<String>,
    configured: Option<Language>,
    locale: Option<&str>,
) -> Language {
    if let Some(value) = explicit {
        return Language::from_override(value);
    }
    if let Some(value) = environment {
        return Language::from_override(&value);
    }
    if let Some(language) = configured {
        return language;
    }
    locale
        .map(Language::from_system_locale)
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

    #[test]
    fn recognizes_supported_codes_strictly() {
        assert_eq!(Language::from_code("en"), Some(Language::En));
        assert_eq!(Language::from_code("zh"), Some(Language::Zh));
        assert_eq!(Language::from_code("ZH"), Some(Language::Zh));
        assert_eq!(Language::from_code("fr"), None);
        assert_eq!(Language::from_code("zh-CN"), None);
        assert_eq!(Language::from_code(""), None);
    }

    #[test]
    fn configured_language_sits_between_environment_and_locale() {
        assert_eq!(
            resolve_precedence(Some("en"), None, Some(Language::Zh), None),
            Language::En,
            "--lang beats config"
        );
        assert_eq!(
            resolve_precedence(Some("fr"), None, Some(Language::Zh), None),
            Language::En,
            "unsupported --lang still beats config"
        );
        assert_eq!(
            resolve_precedence(None, Some("zh".into()), Some(Language::En), None),
            Language::Zh,
            "LKIT_LANG beats config"
        );
        assert_eq!(
            resolve_precedence(None, None, Some(Language::Zh), Some("zh_CN.UTF-8")),
            Language::Zh,
            "config beats system locale"
        );
        assert_eq!(
            resolve_precedence(None, None, Some(Language::En), Some("zh_CN.UTF-8")),
            Language::En,
            "config beats a chinese system locale"
        );
        assert_eq!(
            resolve_precedence(None, None, None, Some("zh_CN.UTF-8")),
            Language::Zh,
            "without config, system locale applies"
        );
        assert_eq!(
            resolve_precedence(None, None, None, None),
            Language::En,
            "no sources fall back to english"
        );
    }
}
