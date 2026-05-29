//! Install use case — TOML config generation and execution.

pub mod config_gen;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lkit_core::{HostInstaller, InstallConfig};

use crate::error::AppError;
use crate::install::config_gen::generate_init_toml;

/// Systemd unit file template for Landscape service.
const SYSTEMD_UNIT_TEMPLATE: &str = r#"[Unit]
Description=Landscape Router
After=network.target

[Service]
Type=simple
ExecStart={home}/landscape-webserver --home {home} --web-root {home}/static
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
"#;

/// Result of a successful installation.
#[derive(Debug)]
pub struct InstallReport {
    /// Landscape HOME directory path.
    pub home: PathBuf,
    /// URL to access the web UI.
    pub web_url: String,
}

/// Executes the installation process: TOML generation, file writing, systemd setup.
pub struct InstallExecutor {
    host_installer: Arc<dyn HostInstaller>,
}

impl InstallExecutor {
    /// Create a new executor with the given host installer.
    pub fn new(host_installer: Arc<dyn HostInstaller>) -> Self {
        Self { host_installer }
    }

    /// Execute the full installation flow.
    ///
    /// 1. Generate `landscape_init.toml` from config
    /// 2. Create Landscape HOME directory
    /// 3. Write `landscape_init.toml` with 0600 permissions
    /// 4. Write systemd unit file
    /// 5. Reload systemd, enable and start the service
    pub async fn execute(
        &self,
        config: &InstallConfig,
        home: &Path,
    ) -> Result<InstallReport, AppError> {
        // 1. Generate TOML
        let toml_content = generate_init_toml(config)?;

        // 2. Create HOME
        self.host_installer
            .create_dir_all(home)
            .await
            .map_err(|e| AppError::Core(e))?;

        // 3. Write landscape_init.toml
        let init_toml_path = home.join("landscape_init.toml");
        self.host_installer
            .write_file(&init_toml_path, toml_content.as_bytes())
            .await
            .map_err(|e| AppError::Core(e))?;

        // 4. Set permissions 0600
        self.host_installer
            .set_permissions(&init_toml_path, 0o600)
            .await
            .map_err(|e| AppError::Core(e))?;

        // 5. Write systemd unit
        let systemd_path = PathBuf::from("/etc/systemd/system/landscape.service");
        let unit_content = SYSTEMD_UNIT_TEMPLATE.replace("{home}", &home.to_string_lossy());
        self.host_installer
            .write_file(&systemd_path, unit_content.as_bytes())
            .await
            .map_err(|e| AppError::Core(e))?;

        // 6. Systemd lifecycle
        self.host_installer
            .daemon_reload()
            .await
            .map_err(|e| AppError::Core(e))?;

        self.host_installer
            .enable_service("landscape.service")
            .await
            .map_err(|e| AppError::Core(e))?;

        self.host_installer
            .start_service("landscape.service")
            .await
            .map_err(|e| AppError::Core(e))?;

        let lan_ip = config
            .network
            .lan
            .as_ref()
            .map(|l| l.gateway.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let web_url = format!("http://{}:{}", lan_ip, config.landscape.web_port);

        Ok(InstallReport {
            home: home.to_path_buf(),
            web_url,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use lkit_core::{CoreError, LandscapeServiceConfig, LanSetup, NetworkSetup, SourceSelection, WanMode, WanSetup};

    /// Records calls in order for verification.
    struct MockHostInstaller {
        calls: Mutex<Vec<String>>,
        /// If set, fail on the nth write_file call.
        fail_on_write_n: Option<usize>,
        write_count: Mutex<usize>,
    }

    impl MockHostInstaller {
        fn new() -> Self {
            Self {
                calls: Mutex::new(vec![]),
                fail_on_write_n: None,
                write_count: Mutex::new(0),
            }
        }

        fn with_failing_write(n: usize) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                fail_on_write_n: Some(n),
                write_count: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl HostInstaller for MockHostInstaller {
        async fn create_dir_all(&self, _path: &Path) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("create_dir_all".to_string());
            Ok(())
        }

        async fn write_file(&self, _path: &Path, _contents: &[u8]) -> Result<(), CoreError> {
            let mut count = self.write_count.lock().unwrap_or_else(|e| e.into_inner());
            *count += 1;
            if self.fail_on_write_n == Some(*count) {
                return Err(CoreError::Internal("mock write failure".to_string()));
            }
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("write_file".to_string());
            Ok(())
        }

        async fn set_permissions(&self, _path: &Path, _mode: u32) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("set_permissions".to_string());
            Ok(())
        }

        async fn daemon_reload(&self) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("daemon_reload".to_string());
            Ok(())
        }

        async fn enable_service(&self, _unit: &str) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("enable_service".to_string());
            Ok(())
        }

        async fn start_service(&self, _unit: &str) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push("start_service".to_string());
            Ok(())
        }
    }

    fn test_config() -> InstallConfig {
        InstallConfig {
            network: NetworkSetup {
                wan: WanSetup {
                    iface_name: "eth0".to_string(),
                    mode: WanMode::Dhcp,
                },
                lan: Some(LanSetup {
                    member_nics: vec!["eth1".to_string()],
                    gateway: std::net::Ipv4Addr::new(192, 168, 5, 1),
                    mask: 24,
                }),
            },
            landscape: LandscapeServiceConfig {
                web_port: 6300,
                admin_user: "root".to_string(),
                admin_pass: "secret".to_string(),
            },
            source: SourceSelection {
                source_name: None,
                version: None,
            },
            landscape_version: "0.19.2".to_string(),
        }
    }

    /// Verify calls happen in the correct order.
    #[tokio::test]
    async fn test_execute_calls_in_order() -> Result<(), Box<dyn std::error::Error>> {
        let mock = Arc::new(MockHostInstaller::new());
        let executor = InstallExecutor::new(mock.clone());
        let home = PathBuf::from("/tmp/test-landscape");

        let config = test_config();
        let report = executor.execute(&config, &home).await?;

        assert_eq!(report.home, home);
        assert!(report.web_url.contains("6300"));

        let calls = mock.calls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            calls.as_slice(),
            &[
                "create_dir_all",
                "write_file",     // landscape_init.toml
                "set_permissions",
                "write_file",     // systemd unit
                "daemon_reload",
                "enable_service",
                "start_service",
            ]
        );
        Ok(())
    }

    /// Write failure stops execution — later steps are not called.
    #[tokio::test]
    async fn test_execute_propagates_write_error() -> Result<(), Box<dyn std::error::Error>> {
        let mock = Arc::new(MockHostInstaller::with_failing_write(1));
        let executor = InstallExecutor::new(mock.clone());
        let home = PathBuf::from("/tmp/test-landscape");

        let config = test_config();
        let result = executor.execute(&config, &home).await;
        assert!(result.is_err());

        let calls = mock.calls.lock().unwrap_or_else(|e| e.into_inner());
        // create_dir_all succeeded, first write_file failed
        assert_eq!(calls.as_slice(), &["create_dir_all"]);
        Ok(())
    }

    /// Verify the generated TOML is valid by parsing it.
    #[tokio::test]
    async fn test_execute_generates_valid_toml() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::Arc;

        struct CapturingInstaller {
            files: Mutex<Vec<(PathBuf, Vec<u8>)>>,
        }

        impl CapturingInstaller {
            fn new() -> Self {
                Self {
                    files: Mutex::new(vec![]),
                }
            }
        }

        #[async_trait]
        impl HostInstaller for CapturingInstaller {
            async fn create_dir_all(&self, _path: &Path) -> Result<(), CoreError> { Ok(()) }
            async fn write_file(&self, path: &Path, contents: &[u8]) -> Result<(), CoreError> {
                self.files.lock().unwrap_or_else(|e| e.into_inner()).push((path.to_path_buf(), contents.to_vec()));
                Ok(())
            }
            async fn set_permissions(&self, _path: &Path, _mode: u32) -> Result<(), CoreError> { Ok(()) }
            async fn daemon_reload(&self) -> Result<(), CoreError> { Ok(()) }
            async fn enable_service(&self, _unit: &str) -> Result<(), CoreError> { Ok(()) }
            async fn start_service(&self, _unit: &str) -> Result<(), CoreError> { Ok(()) }
        }

        let mock = Arc::new(CapturingInstaller::new());
        let executor = InstallExecutor::new(mock.clone());
        let home = PathBuf::from("/tmp/test-landscape");

        let config = test_config();
        executor.execute(&config, &home).await?;

        let files = mock.files.lock().unwrap_or_else(|e| e.into_inner());
        // First file should be landscape_init.toml
        assert_eq!(files[0].0, home.join("landscape_init.toml"));
        let toml_str = String::from_utf8(files[0].1.clone())?;
        // Verify it parses as valid TOML
        let _parsed: toml::Value = toml::from_str(&toml_str)?;

        Ok(())
    }
}
