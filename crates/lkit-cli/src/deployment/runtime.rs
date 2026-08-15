#[cfg(feature = "test-support")]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "test-support")]
use serde::Deserialize;

use super::health::{self, HealthOptions, HttpsDocsProbe, PortCheck};
use super::manager::{Availability, ServiceManager, ServiceManagerKind};
use super::openrc::Openrc;
use super::plan::InstallError;
#[cfg(feature = "test-support")]
use super::process::Protocol;
use super::systemd::Systemd;
use super::sysvinit::Sysvinit;

/// 按探测顺序选择当前主机可用的服务管理器后端。探测全部失败时退回 systemd,
/// 由工作流的 `require_manager` 对不可用环境报 `UnsupportedPlatform`。
pub(crate) fn host_manager() -> Box<dyn ServiceManager + Send + Sync> {
    let systemd = Systemd::host();
    if matches!(systemd.probe(), Availability::Available { .. }) {
        return Box::new(systemd);
    }
    let openrc = Openrc::host();
    if matches!(openrc.probe(), Availability::Available { .. }) {
        return Box::new(openrc);
    }
    let sysvinit = Sysvinit::host();
    if matches!(sysvinit.probe(), Availability::Available { .. }) {
        return Box::new(sysvinit);
    }
    Box::new(systemd)
}

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
    pub service_manager: Box<dyn ServiceManager + Send + Sync>,
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
            service_manager: host_manager(),
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
    pub(crate) fn test_uses_daemon(path: &Path) -> Result<bool, InstallError> {
        let content = std::fs::read(path).map_err(InstallError::Io)?;
        let config: TestRuntimeConfig = serde_json::from_slice(&content).map_err(|error| {
            InstallError::ParameterUsage(format!(
                "invalid test runtime config {}: {error}",
                path.display()
            ))
        })?;
        Ok(matches!(config.execution, TestExecutionPolicy::Daemon))
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
    #[serde(default)]
    manager_kind: TestManagerKind,
    #[serde(default)]
    systemd: Option<TestSystemdConfig>,
    #[serde(default)]
    openrc: Option<TestOpenrcConfig>,
    #[serde(default)]
    sysvinit: Option<TestSysvinitConfig>,
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
        let service_manager: Box<dyn ServiceManager + Send + Sync> = match self.manager_kind {
            TestManagerKind::Systemd => Box::new(Systemd {
                systemctl: self
                    .systemd
                    .as_ref()
                    .ok_or_else(|| {
                        InstallError::ParameterUsage(
                            "test runtime manager_kind systemd requires a systemd block".into(),
                        )
                    })?
                    .systemctl
                    .clone(),
                system_unit_dir: self
                    .systemd
                    .as_ref()
                    .expect("validated above")
                    .system_unit_dir
                    .clone(),
                run_systemd_dir: self
                    .systemd
                    .as_ref()
                    .expect("validated above")
                    .run_systemd_dir
                    .clone(),
                pid1_is_systemd: self
                    .systemd
                    .as_ref()
                    .expect("validated above")
                    .pid1_is_systemd,
                resolv_conf: self
                    .systemd
                    .as_ref()
                    .expect("validated above")
                    .resolv_conf
                    .clone(),
            }),
            TestManagerKind::Openrc => Box::new(Openrc {
                rc_service: self
                    .openrc
                    .as_ref()
                    .ok_or_else(|| {
                        InstallError::ParameterUsage(
                            "test runtime manager_kind openrc requires an openrc block".into(),
                        )
                    })?
                    .rc_service
                    .clone(),
                rc_update: self
                    .openrc
                    .as_ref()
                    .expect("validated above")
                    .rc_update
                    .clone(),
                init_d_dir: self
                    .openrc
                    .as_ref()
                    .expect("validated above")
                    .init_d_dir
                    .clone(),
                resolv_conf: self
                    .openrc
                    .as_ref()
                    .expect("validated above")
                    .resolv_conf
                    .clone(),
            }),
            TestManagerKind::Sysvinit => Box::new(Sysvinit {
                update_rc_d: self
                    .sysvinit
                    .as_ref()
                    .ok_or_else(|| {
                        InstallError::ParameterUsage(
                            "test runtime manager_kind sysvinit requires a sysvinit block".into(),
                        )
                    })?
                    .update_rc_d
                    .clone(),
                init_d_dir: self
                    .sysvinit
                    .as_ref()
                    .expect("validated above")
                    .init_d_dir
                    .clone(),
                rc_d_glob: self
                    .sysvinit
                    .as_ref()
                    .expect("validated above")
                    .rc_d_glob
                    .clone(),
                resolv_conf: self
                    .sysvinit
                    .as_ref()
                    .expect("validated above")
                    .resolv_conf
                    .clone(),
            }),
        };
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
            service_manager,
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
    Daemon,
}

#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TestManagerKind {
    #[default]
    Systemd,
    Openrc,
    Sysvinit,
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
struct TestOpenrcConfig {
    rc_service: PathBuf,
    rc_update: PathBuf,
    init_d_dir: PathBuf,
    resolv_conf: PathBuf,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Deserialize)]
struct TestSysvinitConfig {
    update_rc_d: PathBuf,
    init_d_dir: PathBuf,
    rc_d_glob: PathBuf,
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
