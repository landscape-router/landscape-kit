use std::path::Path;

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

/// 把语言预设写回 `config.toml` 的 `[ui] language`。只有交互控制台按 `L` 切换语言时
/// 调用,CLI 命令只读不写回。用 `toml_edit` 定点修改,保留注释、未知 section/字段与
/// 原有顺序;写回经 tmp + rename 原子完成,并发读方只会看到旧或新文件,不会撕裂。
/// 对单字段偏好,并发切换表现为最后写入者生效,无需安装锁。
///
/// 文件缺失时创建带默认仓库来源与 `[ui] language` 的最小配置,与"文件缺失"的默认
/// 回退语义一致;TOML 损坏时返回错误且不改动原文件,会话内切换仍然生效。
pub(crate) fn write_language(language: Language) -> Result<(), InstallError> {
    let path = layout::territory_config_file();
    let mut document = match std::fs::read_to_string(&path) {
        Ok(text) => text.parse::<toml_edit::DocumentMut>().map_err(|error| {
            InstallError::CorruptedState(format!(
                "{} is not a valid config file: {error}; fix or delete it to switch the language",
                path.display()
            ))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut document = toml_edit::DocumentMut::new();
            document["schema_version"] = toml_edit::value(1i64);
            let mut repository = toml_edit::Table::new();
            repository["kind"] = toml_edit::value("github");
            repository["location"] =
                toml_edit::value(crate::release::repository::github::DEFAULT_REPOSITORY);
            document["repository"] = toml_edit::Item::Table(repository);
            let mut ui = toml_edit::Table::new();
            ui["language"] = toml_edit::value(language.code());
            document["ui"] = toml_edit::Item::Table(ui);
            return atomic_write(&path, &document.to_string());
        }
        Err(error) => {
            return Err(InstallError::Io(std::io::Error::new(
                error.kind(),
                format!("failed to read {}: {error}", path.display()),
            )));
        }
    };
    match document.get_mut("ui") {
        Some(item) if !item.is_table() => {
            *item = toml_edit::Item::Table(toml_edit::Table::new());
        }
        None => {
            document["ui"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        _ => {}
    }
    document["ui"]["language"] = toml_edit::value(language.code());
    atomic_write(&path, &document.to_string())
}

/// 原子写回:独占临时文件写全量内容并 `sync_all`,再 rename 到目标路径。临时文件放在
/// `run/` 下,失败残留也不会被地盘的顶层内容检查当作未知文件。
fn atomic_write(path: &Path, content: &str) -> Result<(), InstallError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let run_dir = layout::territory_run_dir();
    std::fs::create_dir_all(&run_dir).map_err(InstallError::Io)?;
    let tmp = run_dir.join("config.toml.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    if let Err(error) = file.write_all(content.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(InstallError::Io(error));
    }
    if let Err(error) = file.sync_all() {
        let _ = std::fs::remove_file(&tmp);
        return Err(InstallError::Io(error));
    }
    std::fs::rename(&tmp, path).map_err(InstallError::Io)
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

    #[test]
    fn write_language_creates_a_minimal_config_on_missing_file() {
        use std::os::unix::fs::MetadataExt;

        let (_guard, territory) = setup("write-missing");
        write_language(Language::Zh).unwrap();
        assert_eq!(load_language(), Some(Language::Zh));
        assert_eq!(
            load_repository().unwrap().unwrap(),
            github_source(),
            "the created config must keep the default repository semantics"
        );
        let path = territory.join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[ui]"));
        assert!(text.contains("language = \"zh\""));
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o077, 0, "config must be root-only");
        assert!(
            !territory.join("run/config.toml.tmp").exists(),
            "the atomic write must not leave a temp file behind"
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn write_language_updates_the_existing_preset() {
        let (_guard, territory) = setup("write-update");
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
        write_language(Language::Zh).unwrap();
        assert_eq!(load_language(), Some(Language::Zh));
        write_language(Language::En).unwrap();
        assert_eq!(load_language(), Some(Language::En));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn write_language_preserves_unknown_sections_comments_and_order() {
        let (_guard, territory) = setup("write-preserve");
        std::fs::write(
            territory.join("config.toml"),
            br#"# user comment
schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape/"

[future]
key = "value"

[ui]
language = "en"
"#,
        )
        .unwrap();
        write_language(Language::Zh).unwrap();
        let text = std::fs::read_to_string(territory.join("config.toml")).unwrap();
        assert!(text.contains("# user comment"), "comments must survive");
        assert!(text.contains("[future]"), "unknown sections must survive");
        assert!(text.contains("key = \"value\""));
        assert!(text.contains("https://repo.example.com/landscape/"));
        assert!(text.contains("language = \"zh\""));
        assert_eq!(load_language(), Some(Language::Zh));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn write_language_adds_a_missing_ui_section() {
        let (_guard, territory) = setup("write-ui-missing");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"
"#,
        )
        .unwrap();
        write_language(Language::Zh).unwrap();
        assert_eq!(load_language(), Some(Language::Zh));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn write_language_replaces_a_non_table_ui_value() {
        let (_guard, territory) = setup("write-ui-scalar");
        std::fs::write(
            territory.join("config.toml"),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

ui = 42
"#,
        )
        .unwrap();
        write_language(Language::Zh).unwrap();
        assert_eq!(load_language(), Some(Language::Zh));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn write_language_refuses_corrupt_config_without_modifying_it() {
        let (_guard, territory) = setup("write-corrupt");
        std::fs::write(territory.join("config.toml"), b"not toml [[[").unwrap();
        assert!(matches!(
            write_language(Language::Zh),
            Err(InstallError::CorruptedState(_))
        ));
        assert_eq!(
            std::fs::read_to_string(territory.join("config.toml")).unwrap(),
            "not toml [[[",
            "a corrupt config must be left untouched"
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
