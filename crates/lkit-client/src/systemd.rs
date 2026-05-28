//! Systemd service manager — shell-call backend for the ServiceManager trait.

use async_trait::async_trait;
use tokio::process::Command;

use lkit_core::{CoreError, ServiceManager, ServiceState};

/// Manages a systemd service unit via shell commands.
pub struct SystemdManager {
    service_name: String,
}

impl SystemdManager {
    /// Create a manager for the given systemd unit (e.g. "landscape.service").
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }
}

#[async_trait]
impl ServiceManager for SystemdManager {
    async fn status(&self) -> Result<ServiceState, CoreError> {
        // `systemctl is-active` returns non-zero for "inactive" (legitimate state),
        // not just errors. `unwrap_or(false)` treats non-zero as inactive.
        let active = run_cmd("systemctl", &["is-active", &self.service_name])
            .await
            .map(|out| out.trim() == "active")
            .unwrap_or(false);

        let enabled = run_cmd("systemctl", &["is-enabled", &self.service_name])
            .await
            .map(|out| out.trim() == "enabled")
            .unwrap_or(false);

        let pid = run_cmd(
            "systemctl",
            &["show", &self.service_name, "--property=MainPID"],
        )
        .await
        .ok()
        .and_then(|out| {
            let line = out.trim();
            line.strip_prefix("MainPID=")
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|&p| p != 0)
        });

        Ok(ServiceState {
            active,
            enabled,
            pid,
        })
    }

    async fn start(&self) -> Result<(), CoreError> {
        run_cmd("systemctl", &["start", &self.service_name])
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied")
                    || e.to_string().contains("permission")
                {
                    CoreError::Internal(format!(
                        "permission denied: failed to start {}: {}",
                        self.service_name, e
                    ))
                } else {
                    CoreError::Internal(format!(
                        "failed to start {}: {}",
                        self.service_name, e
                    ))
                }
            })?;
        Ok(())
    }

    async fn stop(&self) -> Result<(), CoreError> {
        run_cmd("systemctl", &["stop", &self.service_name])
            .await
            .map_err(|e| {
                CoreError::Internal(format!("failed to stop {}: {}", self.service_name, e))
            })?;
        Ok(())
    }

    async fn restart(&self) -> Result<(), CoreError> {
        run_cmd("systemctl", &["restart", &self.service_name])
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied")
                    || e.to_string().contains("permission")
                {
                    CoreError::Internal(format!(
                        "permission denied: failed to restart {}: {}",
                        self.service_name, e
                    ))
                } else {
                    CoreError::Internal(format!(
                        "failed to restart {}: {}",
                        self.service_name, e
                    ))
                }
            })?;
        Ok(())
    }
}

/// Run an external command and return its stdout. Arguments are passed as an
/// array (no shell interpolation) per CONVENTIONS.md §9.
async fn run_cmd(program: &str, args: &[&str]) -> Result<String, CoreError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| CoreError::Internal(format!("failed to execute {}: {}", program, e)))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CoreError::Internal(format!(
            "{} {} exited with {}: {}",
            program,
            args.join(" "),
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_manager_stores_service_name() {
        let mgr = SystemdManager::new("landscape.service");
        assert_eq!(mgr.service_name, "landscape.service");
    }

    #[test]
    fn systemd_manager_accepts_string() {
        let name = String::from("my.service");
        let mgr = SystemdManager::new(name);
        assert_eq!(mgr.service_name, "my.service");
    }
}
