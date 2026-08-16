use serde::{Deserialize, Serialize};

use super::layout;
use super::plan::{InstallError, RepositoryChoice};
use crate::i18n::Language;

pub(crate) const CONFIG_SCHEMA_VERSION: u64 = 1;

/// 仓库来源记录。state 不再保存来源,该类型只属于配置文件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct RepositorySource {
    pub kind: RepositorySourceKind,
    pub location: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepositorySourceKind {
    Github,
    Http,
}

impl RepositorySource {
    /// 转换为 CLI/计划层统一的仓库选择。规范化的来源可直接解析为 provider。
    pub(crate) fn to_choice(&self) -> RepositoryChoice {
        match self.kind {
            RepositorySourceKind::Github => RepositoryChoice::Github(self.location.clone()),
            RepositorySourceKind::Http => RepositoryChoice::Http(self.location.clone()),
        }
    }
}

/// 界面偏好 section,目前只用于语言预设。与仓库来源不同,这里的值是宽容读取:
/// 缺失、损坏或不受支持都不阻断命令。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct UiSection {
    #[serde(default)]
    pub language: Option<String>,
}

/// 安装根目录顶层的用户可编辑配置文件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ConfigFile {
    pub schema_version: u64,
    pub repository: RepositorySource,
    #[serde(default)]
    pub ui: Option<UiSection>,
}

/// 读取仓库来源配置。文件不存在时返回 `Ok(None)`,调用方按官方 GitHub 默认处理;
/// 内容损坏或不规范时返回 `CorruptedState` 并阻断命令,提示修复或删除该文件。
/// 读取时对来源做校验和规范化:HTTP 位置按 protocol v1 规则补全并校验,
/// GitHub 位置校验 `owner/repo` 格式,与 provider 构造使用同一套规则。
pub(crate) fn load_repository() -> Result<Option<RepositorySource>, InstallError> {
    let path = layout::territory_config_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(InstallError::Io(std::io::Error::new(
                error.kind(),
                format!("failed to read {}: {error}", path.display()),
            )));
        }
    };
    let config: ConfigFile = match toml::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            return Err(InstallError::CorruptedState(format!(
                "{} is not a valid config file: {error}; fix or delete it to fall back to the official GitHub default",
                path.display()
            )));
        }
    };
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(InstallError::CorruptedState(format!(
            "unsupported config schema version {} in {}; fix or delete the file to fall back to the official GitHub default",
            config.schema_version,
            path.display()
        )));
    }
    let provider = crate::release::repository::provider_for(
        match config.repository.kind {
            RepositorySourceKind::Github => crate::release::repository::ProviderKind::Github,
            RepositorySourceKind::Http => crate::release::repository::ProviderKind::Http,
        },
        &config.repository.location,
    )
    .map_err(|error| {
        InstallError::CorruptedState(format!(
            "{} contains an invalid repository source: {error}; fix or delete the file to fall back to the official GitHub default",
            path.display()
        ))
    })?;
    Ok(Some(RepositorySource {
        kind: config.repository.kind,
        location: provider.location().to_string(),
    }))
}

/// 缺省来源解析策略:配置存在且有效时使用记录的来源,文件缺失时回落官方 GitHub。
/// 显式 CLI 来源由调用方直接传入,不经由此处;本函数是"配置 > 官方 GitHub"回退的
/// 唯一实现,首次安装与已安装命令共用。
pub(crate) fn resolve_default_choice() -> Result<RepositoryChoice, InstallError> {
    match load_repository()? {
        Some(source) => Ok(source.to_choice()),
        None => Ok(RepositoryChoice::Github(
            crate::release::repository::github::DEFAULT_REPOSITORY.into(),
        )),
    }
}

/// 读取配置预设的语言。这是**宽容读取**:文件缺失、TOML 损坏、`[ui] language`
/// 缺失或值不受支持(如 `fr`)时一律返回 `None`,由调用方回落系统 locale 或默认
/// 英语,绝不阻断命令。与 `load_repository` 的严格校验不同,语言预设是全局生效的
/// 展示偏好,不能因为配置问题影响任何命令。
pub(crate) fn load_language() -> Option<Language> {
    let path = layout::territory_config_file();
    let text = std::fs::read_to_string(&path).ok()?;
    let config: ConfigFile = toml::from_str(&text).ok()?;
    config
        .ui
        .as_ref()
        .and_then(|ui| ui.language.as_deref())
        .and_then(Language::from_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::layout;

    /// 建立隔离测试现场:返回 (守卫, 地盘)。
    fn setup(name: &str) -> (layout::TerritoryOverride, std::path::PathBuf) {
        let temp =
            std::env::temp_dir().join(format!("lkit-config-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = layout::test_territory(&territory);
        (guard, territory)
    }

    fn github_source() -> RepositorySource {
        RepositorySource {
            kind: RepositorySourceKind::Github,
            location: "ThisSeanZhang/landscape".into(),
        }
    }

    #[test]
    fn missing_config_returns_none() {
        let (_guard, territory) = setup("missing");
        assert!(load_repository().unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn parses_valid_github_config() {
        let (_guard, territory) = setup("github");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"
"#,
        )
        .unwrap();
        assert_eq!(load_repository().unwrap().unwrap(), github_source());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn parses_and_normalizes_http_config() {
        let (_guard, territory) = setup("http");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape"
"#,
        )
        .unwrap();
        assert_eq!(
            load_repository().unwrap().unwrap(),
            RepositorySource {
                kind: RepositorySourceKind::Http,
                location: "https://repo.example.com/landscape/".into(),
            }
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_corrupted_config() {
        let (_guard, territory) = setup("corrupt");
        std::fs::write(territory.join("config.toml"), b"not toml [[[").unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            territory.join("config.toml"),
            b"schema_version = 2\n[repository]\nkind = \"github\"\nlocation = \"x\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            territory.join("config.toml"),
            b"schema_version = 1\n[repository]\nkind = \"mirror\"\nlocation = \"x\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_unsafe_or_malformed_http_config() {
        let (_guard, territory) = setup("unsafe");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "http"
location = "http://example.com/repository"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://example.com/repo?x=1"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "http"
location = "not a url"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_malformed_github_config() {
        let (_guard, territory) = setup("github-bad");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "not-owner-repo"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn ignores_unknown_sections_and_fields() {
        let (_guard, territory) = setup("unknown");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[future]
key = "value"
"#,
        )
        .unwrap();
        assert_eq!(load_repository().unwrap().unwrap(), github_source());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn default_choice_falls_back_to_official_github_without_config() {
        let (_guard, territory) = setup("default-missing");
        assert_eq!(
            resolve_default_choice().unwrap(),
            RepositoryChoice::Github("ThisSeanZhang/landscape".into())
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn default_choice_uses_recorded_source_when_config_present() {
        let (_guard, territory) = setup("default-recorded");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape"
"#,
        )
        .unwrap();
        assert_eq!(
            resolve_default_choice().unwrap(),
            RepositoryChoice::Http("https://repo.example.com/landscape/".into())
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn default_choice_blocks_on_corrupted_config() {
        let (_guard, territory) = setup("default-corrupt");
        std::fs::write(territory.join("config.toml"), b"not toml [[[").unwrap();
        assert!(matches!(
            resolve_default_choice(),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn converts_source_to_repository_choice() {
        assert_eq!(
            github_source().to_choice(),
            RepositoryChoice::Github("ThisSeanZhang/landscape".into())
        );
        assert_eq!(
            RepositorySource {
                kind: RepositorySourceKind::Github,
                location: "Another/landscape".into(),
            }
            .to_choice(),
            RepositoryChoice::Github("Another/landscape".into()),
            "non-default github location must survive the conversion"
        );
        assert_eq!(
            RepositorySource {
                kind: RepositorySourceKind::Http,
                location: "https://repo.example.com/landscape/".into(),
            }
            .to_choice(),
            RepositoryChoice::Http("https://repo.example.com/landscape/".into())
        );
    }

    #[test]
    fn loads_configured_language() {
        let (_guard, territory) = setup("lang-valid");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[ui]
language = "zh"
"#,
        )
        .unwrap();
        assert_eq!(load_language(), Some(Language::Zh));

        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[ui]
language = "en"
"#,
        )
        .unwrap();
        assert_eq!(load_language(), Some(Language::En));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn missing_config_has_no_language_preset() {
        let (_guard, territory) = setup("lang-missing");
        assert_eq!(load_language(), None);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn ignores_unsupported_language_values() {
        let (_guard, territory) = setup("lang-unsupported");
        for language in ["fr", "zh-CN", "42", ""] {
            std::fs::write(
                territory.join("config.toml"),
                format!(
                    "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[ui]\nlanguage = \"{language}\"\n"
                ),
            )
            .unwrap();
            assert_eq!(
                load_language(),
                None,
                "unsupported language {language:?} must be ignored"
            );
        }

        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[ui]
language = "ZH"
"#,
        )
        .unwrap();
        assert_eq!(
            load_language(),
            Some(Language::Zh),
            "case must be normalized like --lang"
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn ignores_missing_ui_section_and_corrupt_config_for_language() {
        let (_guard, territory) = setup("lang-tolerant");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"
"#,
        )
        .unwrap();
        assert_eq!(load_language(), None, "no [ui] section");

        std::fs::write(territory.join("config.toml"), b"not toml [[[").unwrap();
        assert_eq!(
            load_language(),
            None,
            "corrupt config must not block language resolution"
        );

        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[ui]
language = 42
"#,
        )
        .unwrap();
        assert_eq!(
            load_language(),
            None,
            "wrong field type must be ignored for language"
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
