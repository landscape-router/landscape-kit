use std::cell::Cell;
use std::ffi::OsString;
use std::sync::atomic::{AtomicU8, Ordering};

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

    fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "zh" => Some(Self::Zh),
            _ => None,
        }
    }

    fn from_override(value: &str) -> Self {
        Self::from_code(value).unwrap_or(Self::En)
    }

    fn from_system_locale(value: &str) -> Self {
        let primary = value
            .trim()
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or_default();
        match primary.to_ascii_lowercase().as_str() {
            "zh" => Self::Zh,
            _ => Self::En,
        }
    }
}

pub(crate) fn current() -> Language {
    THREAD_LANGUAGE
        .get()
        .unwrap_or_else(|| match LANGUAGE.load(Ordering::SeqCst) {
            ZH => Language::Zh,
            _ => Language::En,
        })
}

pub(crate) fn configure(language: Language) {
    LANGUAGE.store(
        match language {
            Language::En => EN,
            Language::Zh => ZH,
        },
        Ordering::SeqCst,
    );
    THREAD_LANGUAGE.set(Some(language));
}

pub(crate) fn toggle() {
    configure(current().toggled());
}

pub(crate) fn resolve(explicit: Option<&str>) -> Language {
    if let Some(value) = explicit {
        return Language::from_override(value);
    }
    if let Some(value) = std::env::var_os(LANGUAGE_ENV).filter(|value| !value.is_empty()) {
        return value
            .to_str()
            .map(Language::from_override)
            .unwrap_or(Language::En);
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
        .and_then(|value| value.to_str().map(Language::from_system_locale))
        .unwrap_or(Language::En)
}

pub(crate) fn parse_arg(value: &str) -> Result<String, String> {
    Language::from_code(value)
        .map(|language| language.code().to_string())
        .ok_or_else(|| format!("unsupported language '{value}', use en or zh"))
}

pub(crate) fn preconfigure(args: impl IntoIterator<Item = OsString>) {
    let args: Vec<OsString> = args.into_iter().collect();
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        if argument == "--lang" {
            let language = args
                .next()
                .and_then(|value| value.to_str())
                .map(Language::from_override)
                .unwrap_or(Language::En);
            configure(language);
            return;
        }
        if let Some(value) = argument
            .to_str()
            .and_then(|argument| argument.strip_prefix("--lang="))
        {
            configure(Language::from_override(value));
            return;
        }
    }
    configure(resolve(None));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes_and_toggle() {
        assert_eq!(Language::from_code("en"), Some(Language::En));
        assert_eq!(Language::from_code("ZH"), Some(Language::Zh));
        assert_eq!(Language::from_code("fr"), None);
        assert_eq!(Language::En.toggled(), Language::Zh);
    }

    #[test]
    fn system_locale_uses_primary_language() {
        assert_eq!(Language::from_system_locale("zh_CN.UTF-8"), Language::Zh);
        assert_eq!(Language::from_system_locale("en_US.UTF-8"), Language::En);
    }
}
