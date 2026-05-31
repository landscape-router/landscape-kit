//! Status use case — queries systemd and Landscape API for service status.

use std::sync::Arc;

use lkit_core::{LkitClient, ServiceManager, ServiceState};

use crate::AppError;

/// Status report combining local systemd state and API version.
pub struct StatusReport {
    /// Local systemd service state.
    pub service: ServiceState,
    /// Landscape version string, or None if API is unreachable.
    pub landscape_version: Option<String>,
}

/// Queries systemd and the Landscape API to produce a combined status report.
pub struct StatusUseCase {
    client: Arc<dyn LkitClient>,
    service_manager: Arc<dyn ServiceManager>,
}

impl StatusUseCase {
    /// Create a new status use case.
    pub fn new(client: Arc<dyn LkitClient>, service_manager: Arc<dyn ServiceManager>) -> Self {
        Self { client, service_manager }
    }

    /// Execute the status query.
    ///
    /// API failure is not fatal — `landscape_version` will be None in that case.
    pub async fn execute(&self) -> Result<StatusReport, AppError> {
        let service = self.service_manager.status().await?;
        let landscape_version = self.client.get_version().await.ok();
        Ok(StatusReport { service, landscape_version })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use lkit_core::CoreError;

    struct MockServiceManager {
        active: bool,
    }

    #[async_trait]
    impl ServiceManager for MockServiceManager {
        async fn status(&self) -> Result<ServiceState, CoreError> {
            Ok(ServiceState {
                active: self.active,
                enabled: true,
                pid: if self.active { Some(42) } else { None },
            })
        }
        async fn start(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn restart(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct MockLkitClient {
        api_ok: bool,
    }

    #[async_trait]
    impl LkitClient for MockLkitClient {
        async fn get_version(&self) -> Result<String, CoreError> {
            if self.api_ok {
                Ok("1.0.0".into())
            } else {
                Err(CoreError::Internal("connection refused".into()))
            }
        }
        async fn health_check(&self) -> Result<bool, CoreError> {
            Ok(self.api_ok)
        }
        async fn export_config(&self) -> Result<String, CoreError> {
            if self.api_ok {
                Ok("[config]\nversion = \"1.0.0\"".into())
            } else {
                Err(CoreError::Internal("connection refused".into()))
            }
        }
    }

    #[tokio::test]
    async fn status_both_available() -> Result<(), Box<dyn std::error::Error>> {
        let uc = StatusUseCase::new(
            Arc::new(MockLkitClient { api_ok: true }),
            Arc::new(MockServiceManager { active: true }),
        );
        let report = uc.execute().await?;
        assert!(report.service.active);
        let ver = report.landscape_version.ok_or("version should be Some")?;
        assert_eq!(ver, "1.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn status_api_unavailable() -> Result<(), Box<dyn std::error::Error>> {
        let uc = StatusUseCase::new(
            Arc::new(MockLkitClient { api_ok: false }),
            Arc::new(MockServiceManager { active: true }),
        );
        let report = uc.execute().await?;
        assert!(report.service.active);
        assert!(report.landscape_version.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn status_service_inactive() -> Result<(), Box<dyn std::error::Error>> {
        let uc = StatusUseCase::new(
            Arc::new(MockLkitClient { api_ok: true }),
            Arc::new(MockServiceManager { active: false }),
        );
        let report = uc.execute().await?;
        assert!(!report.service.active);
        assert_eq!(report.landscape_version.as_deref(), Some("1.0.0"));
        Ok(())
    }
}
