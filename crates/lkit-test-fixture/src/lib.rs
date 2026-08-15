use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

extern crate self as lkit_test_fixture;

#[path = "bin/lkit-test-init.rs"]
#[allow(dead_code)]
pub mod init_program;
#[path = "bin/landscape-webserver.rs"]
pub mod landscape_program;
#[path = "bin/lkit-test-systemctl.rs"]
pub mod systemctl_program;
pub const FIXTURE_CONFIG_ENV: &str = "LKIT_LANDSCAPE_FIXTURE_CONFIG";
pub const FIXTURE_CONFIG_FILE: &str = "lkit-fixture.json";
pub const SYSTEMCTL_CONFIG_ENV: &str = "LKIT_TEST_SYSTEMCTL_CONFIG";
pub const INIT_CONFIG_ENV: &str = "LKIT_TEST_INIT_CONFIG";
pub const FIXTURE_BUILD_VERSION: Option<&str> = option_env!("LKIT_FIXTURE_BUILD_VERSION");

pub mod contract {
    pub const DOCS_PATH: &str = "/api/docs";
    pub const EXPORT_PATH: &str = "/api/v1/system/config/export";
    pub const INIT_CONFIG: &str = "landscape_init.toml";
    pub const INIT_LOCK: &str = "landscape_init.lock";
    pub const LANDSCAPE_CONFIG: &str = "landscape.toml";
    pub const DATABASE: &str = "landscape_db.sqlite";
    pub const API_TOKEN: &str = "landscape_api_token";
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    #[default]
    Healthy,
    StartExit,
    DelayedReady,
    MissingInitArtifacts,
    HealthError,
    ExportError,
    ExitDuringStability,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct LandscapeFixtureConfig {
    pub schema_version: u64,
    pub scenario: Scenario,
    pub listen_address: IpAddr,
    pub dns_tcp_port: u16,
    pub dns_udp_port: u16,
    pub http_port: u16,
    pub https_port: u16,
    pub ready_delay_ms: u64,
    pub exit_after_ms: u64,
    pub start_exit_code: i32,
    pub export_version: String,
    pub export_content: String,
}

impl Default for LandscapeFixtureConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scenario: Scenario::Healthy,
            listen_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            dns_tcp_port: 10_053,
            dns_udp_port: 10_053,
            http_port: 16_300,
            https_port: 16_443,
            ready_delay_ms: 750,
            exit_after_ms: 2_000,
            start_exit_code: 1,
            export_version: "0.22.0".into(),
            export_content: "version = \"0.22.0\"\n".into(),
        }
    }
}

impl LandscapeFixtureConfig {
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read(path)
            .with_context(|| format!("read Landscape fixture config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&content)
            .with_context(|| format!("parse Landscape fixture config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported fixture schema version"
        );
        anyhow::ensure!(self.dns_tcp_port != 0, "dns_tcp_port must not be zero");
        anyhow::ensure!(self.dns_udp_port != 0, "dns_udp_port must not be zero");
        anyhow::ensure!(self.http_port != 0, "http_port must not be zero");
        anyhow::ensure!(self.https_port != 0, "https_port must not be zero");
        anyhow::ensure!(
            self.export_version.parse::<semver::Version>().is_ok(),
            "export_version must be valid semver"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemctlFixtureConfig {
    pub schema_version: u64,
    pub unit_dir: PathBuf,
    pub state_dir: PathBuf,
    #[serde(default)]
    pub landscape_config: Option<PathBuf>,
    pub log_path: PathBuf,
    #[serde(default)]
    pub call_log: Option<PathBuf>,
    #[serde(default = "default_systemd_version")]
    pub systemd_version: String,
    /// 需要真实拉起的 unit 名列表(如 `lkit.service`);未列出的 unit 只维护
    /// state 标记,不启动真实进程。
    #[serde(default)]
    pub spawn_units: Vec<String>,
}

impl SystemctlFixtureConfig {
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read(path)
            .with_context(|| format!("read systemctl fixture config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&content)
            .with_context(|| format!("parse systemctl fixture config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported fixture schema version"
        );
        anyhow::ensure!(
            !self.systemd_version.trim().is_empty(),
            "systemd_version must not be empty"
        );
        Ok(())
    }
}

fn default_systemd_version() -> String {
    "252.fixture".into()
}

/// 多角色 init 系统替身(`lkit-test-init`)的配置。`state_dir` 保存 enabled
/// 标记与运行中 pid;`init_d_dir`/`rc_d_dir` 是替身操作的系统目录镜像。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitFixtureConfig {
    pub schema_version: u64,
    pub state_dir: PathBuf,
    pub init_d_dir: PathBuf,
    pub rc_d_dir: PathBuf,
    #[serde(default)]
    pub call_log: Option<PathBuf>,
}

impl InitFixtureConfig {
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read(path)
            .with_context(|| format!("read init fixture config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&content)
            .with_context(|| format!("parse init fixture config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "unsupported fixture schema version"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportInitConfigResponse {
    pub filename: String,
    pub version: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LandscapeApiResponse<T> {
    pub data: T,
}

pub fn export_response(
    config: &LandscapeFixtureConfig,
) -> LandscapeApiResponse<ExportInitConfigResponse> {
    export_response_with_content(config, config.export_content.clone())
}

pub fn export_response_with_content(
    config: &LandscapeFixtureConfig,
    content: String,
) -> LandscapeApiResponse<ExportInitConfigResponse> {
    LandscapeApiResponse {
        data: ExportInitConfigResponse {
            filename: format!("landscape_init_v{}.toml", config.export_version),
            version: config.export_version.clone(),
            content,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_landscape_config_is_valid() {
        LandscapeFixtureConfig::default().validate().unwrap();
    }

    #[test]
    fn export_response_matches_contract() {
        let config = LandscapeFixtureConfig::default();
        let response = export_response(&config);
        assert_eq!(response.data.version, "0.22.0");
        assert_eq!(response.data.filename, "landscape_init_v0.22.0.toml");
    }

    #[test]
    fn rejects_zero_ports() {
        let config = LandscapeFixtureConfig {
            https_port: 0,
            ..LandscapeFixtureConfig::default()
        };
        assert!(config.validate().is_err());
    }
}
