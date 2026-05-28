//! Cross-layer data models.

use serde::{Deserialize, Serialize};

/// Current status of the Landscape service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Landscape version, if detected.
    pub landscape_version: Option<String>,
    /// Whether the systemd unit is active (running).
    pub systemd_active: bool,
    /// Whether the systemd unit is enabled (auto-start).
    pub systemd_enabled: bool,
    /// Whether the Landscape API is reachable.
    pub api_reachable: bool,
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
                DiagnosticCheck { name: "a".into(), passed: true, message: "ok".into() },
                DiagnosticCheck { name: "b".into(), passed: true, message: "ok".into() },
            ],
        };
        assert!(result.all_passed());
    }

    #[test]
    fn diagnostic_result_not_all_passed_when_one_fails() {
        let result = DiagnosticResult {
            checks: vec![
                DiagnosticCheck { name: "a".into(), passed: true, message: "ok".into() },
                DiagnosticCheck { name: "b".into(), passed: false, message: "fail".into() },
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
