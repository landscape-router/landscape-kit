//! Status use case — queries systemd and Landscape API for service status.

use std::sync::Arc;

use lkit_core::{LkitClient, ServiceManager, ServiceState, ServiceStatus};

use crate::AppError;

/// Status report combining local systemd state and API state.
pub struct StatusReport {
    /// Local systemd service state.
    pub service: ServiceState,
    /// Landscape API status, or None if API is unreachable.
    pub landscape: Option<ServiceStatus>,
}

/// Queries systemd and the Landscape API to produce a combined status report.
pub struct StatusUseCase {
    client: Arc<dyn LkitClient>,
    service_manager: Arc<dyn ServiceManager>,
}

impl StatusUseCase {
    /// Create a new status use case.
    pub fn new(client: Arc<dyn LkitClient>, service_manager: Arc<dyn ServiceManager>) -> Self {
        Self {
            client,
            service_manager,
        }
    }

    /// Execute the status query.
    ///
    /// API failure is not fatal — `landscape` will be None in that case.
    pub async fn execute(&self) -> Result<StatusReport, AppError> {
        let service = self.service_manager.status().await?;
        let landscape = self.client.get_status().await.ok();
        Ok(StatusReport { service, landscape })
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
        async fn start(&self) -> Result<(), CoreError> { Ok(()) }
        async fn stop(&self) -> Result<(), CoreError> { Ok(()) }
        async fn restart(&self) -> Result<(), CoreError> { Ok(()) }
    }

    struct MockLkitClient {
        api_ok: bool,
    }

    #[async_trait]
    impl LkitClient for MockLkitClient {
        async fn get_status(&self) -> Result<ServiceStatus, CoreError> {
            if self.api_ok {
                Ok(ServiceStatus {
                    landscape_version: Some("1.0.0".into()),
                    systemd_active: true,
                    systemd_enabled: true,
                    api_reachable: true,
                })
            } else {
                Err(CoreError::Internal("connection refused".into()))
            }
        }
        async fn health_check(&self) -> Result<bool, CoreError> {
            Ok(self.api_ok)
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
        assert!(report.landscape.is_some());
        assert_eq!(
            report.landscape.as_ref().unwrap().landscape_version,
            Some("1.0.0".into())
        );
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
        assert!(report.landscape.is_none());
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
        assert!(report.landscape.is_some());
        Ok(())
    }
}
