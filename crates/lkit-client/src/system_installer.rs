//! Concrete implementation of [`HostInstaller`] using the real filesystem and systemd.

use std::path::Path;

use async_trait::async_trait;
use lkit_core::{CoreError, HostInstaller};

/// Production installer that operates on the real host filesystem and systemd.
pub struct SystemInstaller;

impl SystemInstaller {
    /// Create a new `SystemInstaller`.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemInstaller {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HostInstaller for SystemInstaller {
    async fn create_dir_all(&self, path: &Path) -> Result<(), CoreError> {
        tokio::fs::create_dir_all(path).await.map_err(CoreError::Io)
    }

    async fn write_file(&self, path: &Path, contents: &[u8]) -> Result<(), CoreError> {
        tokio::fs::write(path, contents).await.map_err(CoreError::Io)
    }

    async fn set_permissions(&self, path: &Path, mode: u32) -> Result<(), CoreError> {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(CoreError::Io)
    }

    async fn daemon_reload(&self) -> Result<(), CoreError> {
        let status = tokio::process::Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .await
            .map_err(CoreError::Io)?;
        if !status.success() {
            return Err(CoreError::Internal(format!(
                "systemctl daemon-reload exited with {status}"
            )));
        }
        Ok(())
    }

    async fn enable_service(&self, unit: &str) -> Result<(), CoreError> {
        let status = tokio::process::Command::new("systemctl")
            .args(["enable", unit])
            .status()
            .await
            .map_err(CoreError::Io)?;
        if !status.success() {
            return Err(CoreError::Internal(format!(
                "systemctl enable {unit} exited with {status}"
            )));
        }
        Ok(())
    }

    async fn start_service(&self, unit: &str) -> Result<(), CoreError> {
        let status = tokio::process::Command::new("systemctl")
            .args(["start", unit])
            .status()
            .await
            .map_err(CoreError::Io)?;
        if !status.success() {
            return Err(CoreError::Internal(format!(
                "systemctl start {unit} exited with {status}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// Write a file and read it back.
    #[tokio::test]
    async fn test_write_and_read_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file_path = dir.path().join("test.txt");

        let installer = SystemInstaller::new();
        installer
            .write_file(&file_path, b"hello world")
            .await?;

        let content = tokio::fs::read_to_string(&file_path).await?;
        assert_eq!(content, "hello world");
        Ok(())
    }

    /// Set permissions and verify.
    #[tokio::test]
    async fn test_set_permissions() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file_path = dir.path().join("secret.txt");

        let installer = SystemInstaller::new();
        installer.write_file(&file_path, b"secret").await?;
        installer.set_permissions(&file_path, 0o600).await?;

        let metadata = tokio::fs::metadata(&file_path).await?;
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        Ok(())
    }

    /// Create nested directories.
    #[tokio::test]
    async fn test_create_dir_all() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let nested = dir.path().join("a").join("b").join("c");

        let installer = SystemInstaller::new();
        installer.create_dir_all(&nested).await?;

        assert!(nested.exists());
        Ok(())
    }
}
