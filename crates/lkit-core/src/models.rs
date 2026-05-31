//! Cross-layer data models.

use serde::{Deserialize, Serialize};

/// Landscape API v1 通用响应包装。
/// 所有 API 端点返回此结构，业务数据在 `data` 中。
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub data: Option<T>,
    pub error_id: Option<String>,
    pub message: Option<String>,
    pub args: Option<serde_json::Value>,
}

/// `GET /api/v1/system/info` 返回的系统信息。
#[derive(Debug, Deserialize)]
pub struct SystemInfoResponse {
    pub landscape_version: String,
}

/// `GET /api/v1/system/config/export` 返回的导出配置。
#[derive(Debug, Deserialize)]
pub struct ExportInitConfigResponse {
    pub filename: String,
    pub version: String,
    pub content: String,
}

/// Local systemd service state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    /// Whether the service is currently active (running).
    pub active: bool,
    /// Whether the service is enabled (auto-start on boot).
    pub enabled: bool,
    /// Main process PID, if available.
    pub pid: Option<u32>,
}

/// Aggregate result of all diagnostic checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticResult {
    /// Individual check results.
    pub checks: Vec<DiagnosticCheck>,
}

/// A single diagnostic check outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    /// Check identifier, e.g. "systemd", "api", "home".
    pub name: String,
    /// Whether this check passed.
    pub passed: bool,
    /// Human-readable description of the result.
    pub message: String,
}

impl DiagnosticResult {
    /// Returns true only if every check passed.
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_result_all_passed_when_empty() {
        let result = DiagnosticResult { checks: vec![] };
        assert!(result.all_passed());
    }

    #[test]
    fn diagnostic_result_all_passed_when_all_pass() {
        let result = DiagnosticResult {
            checks: vec![
                DiagnosticCheck {
                    name: "a".into(),
                    passed: true,
                    message: "ok".into(),
                },
                DiagnosticCheck {
                    name: "b".into(),
                    passed: true,
                    message: "ok".into(),
                },
            ],
        };
        assert!(result.all_passed());
    }

    #[test]
    fn diagnostic_result_not_all_passed_when_one_fails() {
        let result = DiagnosticResult {
            checks: vec![
                DiagnosticCheck {
                    name: "a".into(),
                    passed: true,
                    message: "ok".into(),
                },
                DiagnosticCheck {
                    name: "b".into(),
                    passed: false,
                    message: "fail".into(),
                },
            ],
        };
        assert!(!result.all_passed());
    }

    #[test]
    fn service_state_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let state = ServiceState { active: true, enabled: false, pid: Some(1234) };
        let json = serde_json::to_string(&state)?;
        let decoded: ServiceState = serde_json::from_str(&json)?;
        assert!(decoded.active);
        assert!(!decoded.enabled);
        assert_eq!(decoded.pid, Some(1234));
        Ok(())
    }

    #[test]
    fn diagnostic_check_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let check = DiagnosticCheck {
            name: "systemd".into(),
            passed: true,
            message: "service running".into(),
        };
        let json = serde_json::to_string(&check)?;
        let decoded: DiagnosticCheck = serde_json::from_str(&json)?;
        assert_eq!(decoded.name, "systemd");
        assert!(decoded.passed);
        Ok(())
    }
}
