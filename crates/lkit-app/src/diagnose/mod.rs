//! Diagnose use case — runs health checks on the Landscape installation.

use std::sync::Arc;

use lkit_core::{DiagnosticCheck, DiagnosticResult, LandscapePaths, LkitClient, ServiceManager};

use crate::AppError;

/// Runs diagnostic checks: systemd state, HOME integrity, API reachability.
pub struct DiagnoseUseCase {
    client: Arc<dyn LkitClient>,
    service_manager: Arc<dyn ServiceManager>,
    landscape_paths: LandscapePaths,
}

impl DiagnoseUseCase {
    /// Create a new diagnose use case.
    pub fn new(
        client: Arc<dyn LkitClient>,
        service_manager: Arc<dyn ServiceManager>,
        landscape_paths: LandscapePaths,
    ) -> Self {
        Self {
            client,
            service_manager,
            landscape_paths,
        }
    }

    /// Execute all diagnostic checks. Each check is independent — a failure
    /// in one does not prevent the others from running.
    pub async fn execute(&self) -> Result<DiagnosticResult, AppError> {
        let mut checks = Vec::new();

        // 1. systemd check
        checks.push(check_systemd(&self.service_manager).await);

        // 2. HOME integrity check
        checks.push(check_home(&self.landscape_paths));

        // 3. API reachability check
        checks.push(check_api(&self.client).await);

        Ok(DiagnosticResult { checks })
    }
}

async fn check_systemd(sm: &Arc<dyn ServiceManager>) -> DiagnosticCheck {
    match sm.status().await {
        Ok(state) => DiagnosticCheck {
            name: "systemd".into(),
            passed: state.active,
            message: if state.active {
                "systemd service is running".into()
            } else {
                "systemd service is not running".into()
            },
        },
        Err(e) => DiagnosticCheck {
            name: "systemd".into(),
            passed: false,
            message: format!("failed to query systemd: {e}"),
        },
    }
}

fn check_home(paths: &LandscapePaths) -> DiagnosticCheck {
    let mut missing = Vec::new();
    if !paths.landscape_config.exists() {
        missing.push("landscape.toml");
    }
    if !paths.db_file.exists() {
        missing.push("landscape_db.sqlite");
    }
    if !paths.static_dir.exists() {
        missing.push("static/");
    }

    if missing.is_empty() {
        DiagnosticCheck {
            name: "home".into(),
            passed: true,
            message: "Landscape HOME is intact".into(),
        }
    } else {
        DiagnosticCheck {
            name: "home".into(),
            passed: false,
            message: format!("missing: {}", missing.join(", ")),
        }
    }
}

async fn check_api(client: &Arc<dyn LkitClient>) -> DiagnosticCheck {
    match client.health_check().await {
        Ok(true) => DiagnosticCheck {
            name: "api".into(),
            passed: true,
            message: "Landscape API is reachable".into(),
        },
        Ok(false) => DiagnosticCheck {
            name: "api".into(),
            passed: false,
            message: "Landscape API health check failed".into(),
        },
        Err(e) => DiagnosticCheck {
            name: "api".into(),
            passed: false,
            message: format!("Landscape API unreachable: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use lkit_core::{CoreError, ServiceState, ServiceStatus};

    struct MockServiceManager {
        active: bool,
    }

    #[async_trait]
    impl ServiceManager for MockServiceManager {
        async fn status(&self) -> Result<ServiceState, CoreError> {
            Ok(ServiceState {
                active: self.active,
                enabled: true,
                pid: None,
            })
        }
        async fn start(&self) -> Result<(), CoreError> { Ok(()) }
        async fn stop(&self) -> Result<(), CoreError> { Ok(()) }
        async fn restart(&self) -> Result<(), CoreError> { Ok(()) }
    }

    struct MockLkitClient {
        healthy: bool,
    }

    #[async_trait]
    impl LkitClient for MockLkitClient {
        async fn get_status(&self) -> Result<ServiceStatus, CoreError> {
            Ok(ServiceStatus {
                landscape_version: None,
                systemd_active: false,
                systemd_enabled: false,
                api_reachable: self.healthy,
            })
        }
        async fn health_check(&self) -> Result<bool, CoreError> {
            Ok(self.healthy)
        }
    }

    #[tokio::test]
    async fn diagnose_all_pass() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("landscape.toml"), "")?;
        std::fs::write(dir.path().join("landscape_db.sqlite"), "")?;
        std::fs::create_dir(dir.path().join("static"))?;

        let paths = LandscapePaths::new(dir.path().to_path_buf());
        let uc = DiagnoseUseCase::new(
            Arc::new(MockLkitClient { healthy: true }),
            Arc::new(MockServiceManager { active: true }),
            paths,
        );
        let result = uc.execute().await?;
        assert!(result.all_passed(), "{:?}", result.checks);
        assert_eq!(result.checks.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn diagnose_detects_missing_home() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = LandscapePaths::new(dir.path().to_path_buf());
        let uc = DiagnoseUseCase::new(
            Arc::new(MockLkitClient { healthy: true }),
            Arc::new(MockServiceManager { active: true }),
            paths,
        );
        let result = uc.execute().await?;
        let home_check = result
            .checks
            .iter()
            .find(|c| c.name == "home")
            .expect("home check should exist");
        assert!(!home_check.passed);
        assert!(home_check.message.contains("missing"));
        Ok(())
    }

    #[tokio::test]
    async fn diagnose_detects_api_down() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("landscape.toml"), "")?;
        std::fs::write(dir.path().join("landscape_db.sqlite"), "")?;
        std::fs::create_dir(dir.path().join("static"))?;

        let paths = LandscapePaths::new(dir.path().to_path_buf());
        let uc = DiagnoseUseCase::new(
            Arc::new(MockLkitClient { healthy: false }),
            Arc::new(MockServiceManager { active: true }),
            paths,
        );
        let result = uc.execute().await?;
        assert!(!result.all_passed());
        let api_check = result
            .checks
            .iter()
            .find(|c| c.name == "api")
            .expect("api check should exist");
        assert!(!api_check.passed);
        Ok(())
    }
}
