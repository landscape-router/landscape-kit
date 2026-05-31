//! Service use case — start, stop, restart the Landscape service.

use std::sync::Arc;

use lkit_core::ServiceManager;

use crate::AppError;

/// Controls the Landscape systemd service.
pub struct ServiceUseCase {
    service_manager: Arc<dyn ServiceManager>,
}

impl ServiceUseCase {
    /// Create a new service use case.
    pub fn new(service_manager: Arc<dyn ServiceManager>) -> Self {
        Self { service_manager }
    }

    /// Start the service.
    pub async fn start(&self) -> Result<(), AppError> {
        self.service_manager.start().await?;
        Ok(())
    }

    /// Stop the service.
    pub async fn stop(&self) -> Result<(), AppError> {
        self.service_manager.stop().await?;
        Ok(())
    }

    /// Restart the service.
    pub async fn restart(&self) -> Result<(), AppError> {
        self.service_manager.restart().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use lkit_core::{CoreError, ServiceState};
    use std::sync::Mutex;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Action {
        Start,
        Stop,
        Restart,
    }

    struct MockServiceManager {
        calls: Mutex<Vec<Action>>,
        should_fail: bool,
    }

    #[async_trait]
    impl ServiceManager for MockServiceManager {
        async fn status(&self) -> Result<ServiceState, CoreError> {
            Ok(ServiceState { active: false, enabled: true, pid: None })
        }
        async fn start(&self) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(Action::Start);
            if self.should_fail { Err(CoreError::Internal("denied".into())) } else { Ok(()) }
        }
        async fn stop(&self) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(Action::Stop);
            if self.should_fail { Err(CoreError::Internal("denied".into())) } else { Ok(()) }
        }
        async fn restart(&self) -> Result<(), CoreError> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(Action::Restart);
            if self.should_fail { Err(CoreError::Internal("denied".into())) } else { Ok(()) }
        }
    }

    #[tokio::test]
    async fn start_delegates_to_manager() -> Result<(), Box<dyn std::error::Error>> {
        let mgr = Arc::new(MockServiceManager { calls: Mutex::new(vec![]), should_fail: false });
        let uc = ServiceUseCase::new(mgr.clone());
        uc.start().await?;
        assert_eq!(*mgr.calls.lock().unwrap_or_else(|e| e.into_inner()), vec![Action::Start]);
        Ok(())
    }

    #[tokio::test]
    async fn stop_delegates_to_manager() -> Result<(), Box<dyn std::error::Error>> {
        let mgr = Arc::new(MockServiceManager { calls: Mutex::new(vec![]), should_fail: false });
        let uc = ServiceUseCase::new(mgr.clone());
        uc.stop().await?;
        assert_eq!(*mgr.calls.lock().unwrap_or_else(|e| e.into_inner()), vec![Action::Stop]);
        Ok(())
    }

    #[tokio::test]
    async fn restart_delegates_to_manager() -> Result<(), Box<dyn std::error::Error>> {
        let mgr = Arc::new(MockServiceManager { calls: Mutex::new(vec![]), should_fail: false });
        let uc = ServiceUseCase::new(mgr.clone());
        uc.restart().await?;
        assert_eq!(*mgr.calls.lock().unwrap_or_else(|e| e.into_inner()), vec![Action::Restart]);
        Ok(())
    }

    #[tokio::test]
    async fn start_propagates_error() {
        let mgr = Arc::new(MockServiceManager { calls: Mutex::new(vec![]), should_fail: true });
        let uc = ServiceUseCase::new(mgr.clone());
        let result = uc.start().await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("denied"));
    }
}
