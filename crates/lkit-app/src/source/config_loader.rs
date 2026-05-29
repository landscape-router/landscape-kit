//! Load source configurations from `lkit.toml`.

use std::path::Path;

use lkit_core::SourceConfig;

use crate::error::AppError;

/// Load source configs from `{manager_home}/config/lkit.toml`.
///
/// Returns an empty list if the file does not exist.
/// Returns `AppError::ConfigGeneration` on parse failure.
pub fn load_lkit_toml(manager_home: &Path) -> Result<Vec<SourceConfig>, AppError> {
    let path = manager_home.join("config").join("lkit.toml");
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| AppError::ConfigGeneration(format!("读取 lkit.toml 失败: {e}")))?;

    #[derive(serde::Deserialize)]
    struct LkitToml {
        sources: Option<Vec<SourceConfig>>,
    }

    let parsed: LkitToml =
        toml::from_str(&content).map_err(|e| AppError::ConfigGeneration(format!("lkit.toml 语法错误: {e}")))?;

    Ok(parsed.sources.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_empty() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let result = load_lkit_toml(dir.path())?;
        assert!(result.is_empty());
        Ok(())
    }

    #[test]
    fn empty_toml_returns_empty() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_dir = dir.path().join("config");
        std::fs::create_dir(&config_dir)?;
        std::fs::write(config_dir.join("lkit.toml"), "")?;
        let result = load_lkit_toml(dir.path())?;
        assert!(result.is_empty());
        Ok(())
    }

    #[test]
    fn valid_toml_returns_sources() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config_dir = dir.path().join("config");
        std::fs::create_dir(&config_dir)?;
        std::fs::write(
            config_dir.join("lkit.toml"),
            r#"
[[sources]]
name = "my-mirror"
type = "http"
priority = 5
base_url = "https://mirror.example.com/landscape"
"#,
        )?;
        let result = load_lkit_toml(dir.path())?;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "my-mirror");
        Ok(())
    }

    #[test]
    fn invalid_toml_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_dir = dir.path().join("config");
        std::fs::create_dir(&config_dir).expect("mkdir");
        let content = "not = valid = toml";
        std::fs::write(config_dir.join("lkit.toml"), content).expect("write");
        let result = load_lkit_toml(dir.path());
        assert!(result.is_err());
    }
}
