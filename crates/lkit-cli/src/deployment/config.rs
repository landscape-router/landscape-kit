use serde::{Deserialize, Serialize};

use super::plan::{InstallError, RepositoryChoice};
use super::root::InstallRoot;

pub(crate) const CONFIG_FILE: &str = "config.toml";
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

/// 安装根目录顶层的用户可编辑配置文件。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ConfigFile {
    pub schema_version: u64,
    pub repository: RepositorySource,
}

/// 读取仓库来源配置。文件不存在时返回 `Ok(None)`,调用方按官方 GitHub 默认处理;
/// 内容损坏或不规范时返回 `CorruptedState` 并阻断命令,提示修复或删除该文件。
/// 读取时对来源做校验和规范化:HTTP 位置按 protocol v1 规则补全并校验,
/// GitHub 位置校验 `owner/repo` 格式,与 provider 构造使用同一套规则。
pub(crate) fn load_repository(
    root: &InstallRoot,
) -> Result<Option<RepositorySource>, InstallError> {
    let path = root.canonical.join(CONFIG_FILE);
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
pub(crate) fn resolve_default_choice(root: &InstallRoot) -> Result<RepositoryChoice, InstallError> {
    match load_repository(root)? {
        Some(source) => Ok(source.to_choice()),
        None => Ok(RepositoryChoice::Github(
            crate::release::repository::github::DEFAULT_REPOSITORY.into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-config-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: std::fs::canonicalize(path).unwrap(),
        }
    }

    fn github_source() -> RepositorySource {
        RepositorySource {
            kind: RepositorySourceKind::Github,
            location: "ThisSeanZhang/landscape".into(),
        }
    }

    #[test]
    fn missing_config_returns_none() {
        let temp = temp_root("missing");
        let root = new_root(&temp);
        assert!(load_repository(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn parses_valid_github_config() {
        let temp = temp_root("github");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"
"#,
        )
        .unwrap();
        assert_eq!(load_repository(&root).unwrap().unwrap(), github_source());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn parses_and_normalizes_http_config() {
        let temp = temp_root("http");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape"
"#,
        )
        .unwrap();
        assert_eq!(
            load_repository(&root).unwrap().unwrap(),
            RepositorySource {
                kind: RepositorySourceKind::Http,
                location: "https://repo.example.com/landscape/".into(),
            }
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_corrupted_config() {
        let temp = temp_root("corrupt");
        let root = new_root(&temp);
        std::fs::write(temp.join(CONFIG_FILE), b"not toml [[[").unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            temp.join(CONFIG_FILE),
            b"schema_version = 2\n[repository]\nkind = \"github\"\nlocation = \"x\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            temp.join(CONFIG_FILE),
            b"schema_version = 1\n[repository]\nkind = \"mirror\"\nlocation = \"x\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_unsafe_or_malformed_http_config() {
        let temp = temp_root("unsafe");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "http"
location = "http://example.com/repository"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://example.com/repo?x=1"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "http"
location = "not a url"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_malformed_github_config() {
        let temp = temp_root("github-bad");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "github"
location = "not-owner-repo"
"#,
        )
        .unwrap();
        assert!(matches!(
            load_repository(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_unknown_sections_and_fields() {
        let temp = temp_root("unknown");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "github"
location = "ThisSeanZhang/landscape"

[future]
key = "value"
"#,
        )
        .unwrap();
        assert_eq!(load_repository(&root).unwrap().unwrap(), github_source());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn default_choice_falls_back_to_official_github_without_config() {
        let temp = temp_root("default-missing");
        let root = new_root(&temp);
        assert_eq!(
            resolve_default_choice(&root).unwrap(),
            RepositoryChoice::Github("ThisSeanZhang/landscape".into())
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn default_choice_uses_recorded_source_when_config_present() {
        let temp = temp_root("default-recorded");
        let root = new_root(&temp);
        std::fs::write(
            temp.join(CONFIG_FILE),
            br#"schema_version = 1

[repository]
kind = "http"
location = "https://repo.example.com/landscape"
"#,
        )
        .unwrap();
        assert_eq!(
            resolve_default_choice(&root).unwrap(),
            RepositoryChoice::Http("https://repo.example.com/landscape/".into())
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn default_choice_blocks_on_corrupted_config() {
        let temp = temp_root("default-corrupt");
        let root = new_root(&temp);
        std::fs::write(temp.join(CONFIG_FILE), b"not toml [[[").unwrap();
        assert!(matches!(
            resolve_default_choice(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
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
}
