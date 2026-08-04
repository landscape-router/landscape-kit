#[cfg(feature = "test-support")]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "test-support")]
use serde::Deserialize;

use super::health::{self, HealthOptions, HttpsDocsProbe, PortCheck};
use super::plan::InstallError;
#[cfg(feature = "test-support")]
use super::process::Protocol;
use super::systemd::Systemd;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreflightPolicy {
    Full,
    Skip,
}

pub(crate) struct InstallRuntime {
    pub allow_non_root: bool,
    pub preflight: PreflightPolicy,
    pub managed_uid: u32,
    pub os_release_path: PathBuf,
    pub sys_class_net: PathBuf,
    pub ip_command: PathBuf,
    pub selinux_fs_path: PathBuf,
    pub selinux_config_path: PathBuf,
    pub network_confirm_timeout: Duration,
    pub test_runtime_path: Option<PathBuf>,
    pub systemd: Systemd,
    pub export_base_url: String,
    pub health_base_url: String,
    pub(crate) health_ports: Vec<PortCheck>,
    startup_timeout: Duration,
    stable_duration: Duration,
}

impl InstallRuntime {
    pub(crate) fn production() -> Self {
        Self {
            allow_non_root: false,
            preflight: PreflightPolicy::Full,
            managed_uid: 0,
            os_release_path: PathBuf::from("/etc/os-release"),
            sys_class_net: PathBuf::from("/sys/class/net"),
            ip_command: PathBuf::from("/usr/sbin/ip"),
            selinux_fs_path: PathBuf::from("/sys/fs/selinux"),
            selinux_config_path: PathBuf::from("/etc/selinux/config"),
            network_confirm_timeout: Duration::from_secs(600),
            test_runtime_path: None,
            systemd: Systemd::host(),
            export_base_url: "https://127.0.0.1:6443".into(),
            health_base_url: "https://127.0.0.1:6443".into(),
            health_ports: health::default_port_checks(),
            startup_timeout: health::STARTUP_TIMEOUT,
            stable_duration: health::STABLE_OBSERVATION,
        }
    }

    pub(crate) fn health_options(&self) -> Result<HealthOptions<HttpsDocsProbe>, InstallError> {
        Ok(HealthOptions {
            docs: HttpsDocsProbe::new(&self.health_base_url)?,
            ports: self.health_ports.clone(),
            startup_timeout: self.startup_timeout,
            stable_duration: self.stable_duration,
        })
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn from_test_file(path: &Path) -> Result<Self, InstallError> {
        let content = std::fs::read(path).map_err(InstallError::Io)?;
        let config: TestRuntimeConfig = serde_json::from_slice(&content).map_err(|error| {
            InstallError::ParameterUsage(format!(
                "invalid test runtime config {}: {error}",
                path.display()
            ))
        })?;
        config.build(path)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn test_uses_systemd_worker(path: &Path) -> Result<bool, InstallError> {
        let content = std::fs::read(path).map_err(InstallError::Io)?;
        let config: TestRuntimeConfig = serde_json::from_slice(&content).map_err(|error| {
            InstallError::ParameterUsage(format!(
                "invalid test runtime config {}: {error}",
                path.display()
            ))
        })?;
        Ok(matches!(
            config.execution,
            TestExecutionPolicy::SystemdWorker
        ))
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug, Deserialize)]
struct TestRuntimeConfig {
    schema_version: u64,
    allow_non_root: bool,
    #[serde(default)]
    preflight: TestPreflightPolicy,
    #[serde(default)]
    execution: TestExecutionPolicy,
    managed_uid: u32,
    os_release_path: PathBuf,
    #[serde(default = "default_sys_class_net")]
    sys_class_net: PathBuf,
    #[serde(default = "default_ip_command")]
    ip_command: PathBuf,
    #[serde(default = "default_selinux_fs")]
    selinux_fs_path: PathBuf,
    #[serde(default = "default_selinux_config")]
    selinux_config_path: PathBuf,
    #[serde(default = "default_network_confirm_timeout_ms")]
    network_confirm_timeout_ms: u64,
    systemd: TestSystemdConfig,
    health: TestHealthConfig,
    export_base_url: String,
}

#[cfg(feature = "test-support")]
impl TestRuntimeConfig {
    fn build(self, source_path: &Path) -> Result<InstallRuntime, InstallError> {
        if self.schema_version != 1 {
            return Err(InstallError::ParameterUsage(format!(
                "unsupported test runtime schema version {}",
                self.schema_version
            )));
        }
        self.health.validate()?;
        if self.export_base_url.trim().is_empty() {
            return Err(InstallError::ParameterUsage(
                "test runtime export_base_url must not be empty".into(),
            ));
        }
        Ok(InstallRuntime {
            allow_non_root: self.allow_non_root,
            preflight: self.preflight.into(),
            managed_uid: self.managed_uid,
            os_release_path: self.os_release_path,
            sys_class_net: self.sys_class_net,
            ip_command: self.ip_command,
            selinux_fs_path: self.selinux_fs_path,
            selinux_config_path: self.selinux_config_path,
            network_confirm_timeout: Duration::from_millis(self.network_confirm_timeout_ms),
            test_runtime_path: Some(source_path.to_path_buf()),
            systemd: Systemd {
                systemctl: self.systemd.systemctl,
                system_unit_dir: self.systemd.system_unit_dir,
                run_systemd_dir: self.systemd.run_systemd_dir,
                pid1_is_systemd: self.systemd.pid1_is_systemd,
                resolv_conf: self.systemd.resolv_conf,
            },
            export_base_url: self.export_base_url,
            health_base_url: self.health.base_url,
            health_ports: vec![
                PortCheck {
                    protocol: Protocol::Tcp,
                    port: self.health.dns_tcp_port,
                },
                PortCheck {
                    protocol: Protocol::Udp,
                    port: self.health.dns_udp_port,
                },
                PortCheck {
                    protocol: Protocol::Tcp,
                    port: self.health.http_port,
                },
                PortCheck {
                    protocol: Protocol::Tcp,
                    port: self.health.https_port,
                },
            ],
            startup_timeout: Duration::from_millis(self.health.startup_timeout_ms),
            stable_duration: Duration::from_millis(self.health.stable_duration_ms),
        })
    }
}

#[cfg(feature = "test-support")]
fn default_sys_class_net() -> PathBuf {
    PathBuf::from("/sys/class/net")
}

#[cfg(feature = "test-support")]
fn default_ip_command() -> PathBuf {
    PathBuf::from("/usr/sbin/ip")
}

#[cfg(feature = "test-support")]
fn default_selinux_fs() -> PathBuf {
    PathBuf::from("/sys/fs/selinux")
}

#[cfg(feature = "test-support")]
fn default_selinux_config() -> PathBuf {
    PathBuf::from("/etc/selinux/config")
}

#[cfg(feature = "test-support")]
fn default_network_confirm_timeout_ms() -> u64 {
    600_000
}

#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TestPreflightPolicy {
    #[default]
    Full,
    Skip,
}

#[cfg(feature = "test-support")]
impl From<TestPreflightPolicy> for PreflightPolicy {
    fn from(value: TestPreflightPolicy) -> Self {
        match value {
            TestPreflightPolicy::Full => Self::Full,
            TestPreflightPolicy::Skip => Self::Skip,
        }
    }
}

#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TestExecutionPolicy {
    #[default]
    Inline,
    SystemdWorker,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Deserialize)]
struct TestSystemdConfig {
    systemctl: PathBuf,
    system_unit_dir: PathBuf,
    run_systemd_dir: PathBuf,
    pid1_is_systemd: bool,
    resolv_conf: PathBuf,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Deserialize)]
struct TestHealthConfig {
    base_url: String,
    dns_tcp_port: u16,
    dns_udp_port: u16,
    http_port: u16,
    https_port: u16,
    startup_timeout_ms: u64,
    stable_duration_ms: u64,
}

#[cfg(feature = "test-support")]
impl TestHealthConfig {
    fn validate(&self) -> Result<(), InstallError> {
        if self.base_url.trim().is_empty() {
            return Err(InstallError::ParameterUsage(
                "test runtime health base_url must not be empty".into(),
            ));
        }
        if [
            self.dns_tcp_port,
            self.dns_udp_port,
            self.http_port,
            self.https_port,
        ]
        .contains(&0)
        {
            return Err(InstallError::ParameterUsage(
                "test runtime health ports must not be zero".into(),
            ));
        }
        if self.startup_timeout_ms == 0 || self.stable_duration_ms == 0 {
            return Err(InstallError::ParameterUsage(
                "test runtime health durations must not be zero".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_health_port() {
        let health = TestHealthConfig {
            base_url: "https://127.0.0.1:16443".into(),
            dns_tcp_port: 0,
            dns_udp_port: 1053,
            http_port: 16300,
            https_port: 16443,
            startup_timeout_ms: 1000,
            stable_duration_ms: 1000,
        };
        assert!(health.validate().is_err());
    }
}
