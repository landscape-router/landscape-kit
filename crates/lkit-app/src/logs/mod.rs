//! Logs use case — reads recent log lines.

use std::sync::Arc;

use lkit_core::LogReader;

use crate::AppError;

/// Reads recent log lines from the Landscape log directory.
pub struct LogsUseCase {
    log_reader: Arc<dyn LogReader>,
}

impl LogsUseCase {
    /// Create a new logs use case.
    pub fn new(log_reader: Arc<dyn LogReader>) -> Self {
        Self { log_reader }
    }

    /// Read the most recent `lines` log lines.
    pub async fn recent(&self, lines: usize) -> Result<Vec<String>, AppError> {
        let result = self.log_reader.recent_lines(lines).await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use lkit_core::CoreError;

    struct MockLogReader {
        lines: Vec<String>,
    }

    #[async_trait]
    impl LogReader for MockLogReader {
        async fn recent_lines(&self, _lines: usize) -> Result<Vec<String>, CoreError> {
            Ok(self.lines.clone())
        }
    }

    struct FailingLogReader;

    #[async_trait]
    impl LogReader for FailingLogReader {
        async fn recent_lines(&self, _lines: usize) -> Result<Vec<String>, CoreError> {
            Err(CoreError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no log dir")))
        }
    }

    #[tokio::test]
    async fn recent_returns_lines() -> Result<(), Box<dyn std::error::Error>> {
        let reader = Arc::new(MockLogReader { lines: vec!["line1".into(), "line2".into()] });
        let uc = LogsUseCase::new(reader);
        let result = uc.recent(50).await?;
        assert_eq!(result, vec!["line1", "line2"]);
        Ok(())
    }

    #[tokio::test]
    async fn recent_propagates_error() {
        let reader = Arc::new(FailingLogReader);
        let uc = LogsUseCase::new(reader);
        let result = uc.recent(50).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("no log dir"));
    }
}
