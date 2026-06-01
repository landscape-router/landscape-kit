//! Restore flow — foreground verification + detached health check with auto-rollback.

use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lkit_core::{BackupEntry, BackupScope};

use crate::AppError;

use super::BackupUseCase;
use super::packer::extract_verified;

impl BackupUseCase {
    /// Build the full recovery snapshot path for a given backup ID.
    fn recovery_path(&self, backup_id: &str) -> PathBuf {
        let parent = self.landscape_paths.home.parent().unwrap_or(&self.landscape_paths.home);
        parent.join(format!(
            "{}.recovery-{backup_id}",
            self.landscape_paths.home.file_name().unwrap_or_default().to_string_lossy()
        ))
    }

    /// Read the HTTPS port from landscape.toml, or default to 6443.
    fn read_https_port(&self) -> u16 {
        let Ok(content) = fs::read_to_string(&self.landscape_paths.landscape_config) else {
            return 6443;
        };
        let Ok(parsed) = toml::from_str::<toml::Value>(&content) else {
            return 6443;
        };
        parsed
            .get("web")
            .and_then(|w| w.get("https_port").or(w.get("port")))
            .and_then(|v| v.as_integer())
            .map(|p| p as u16)
            .unwrap_or(6443)
    }

    /// TCP connect health check with total timeout (tokio async).
    async fn health_check(&self, port: u16, total_timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + total_timeout;
        let addr = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port);

        loop {
            match tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr))
                .await
            {
                Ok(Ok(_)) => return true,
                _ => {
                    if tokio::time::Instant::now() >= deadline {
                        return false;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    /// Foreground phase: verify, extract, stop, replace files, then hint and return.
    pub async fn restore_foreground(&self, entry: &BackupEntry) -> Result<(), AppError> {
        let staging_dir = self.build_staging_dir()?;

        // 1. Space precheck for extraction
        let file_size =
            fs::metadata(&entry.path).map_err(|e| AppError::Backup(format!("stat: {e}")))?.len();
        let extract_need = file_size.saturating_mul(2);
        super::check_space(extract_need, &staging_dir)?;

        // 2. Verify + extract to staging (streaming)
        {
            let mut file =
                fs::File::open(&entry.path).map_err(|e| AppError::Backup(format!("open: {e}")))?;
            extract_verified(&mut file, &entry.checksum, &staging_dir)?;
        }

        // 3. Stop Landscape with retries
        for i in 0..3 {
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            if self.service_manager.stop().await.is_ok() {
                break;
            }
        }

        let recovery_dir = self.recovery_path(&entry.backup_id);

        // 4. mv LANDSCAPE_HOME -> recovery
        if self.landscape_paths.home.exists() {
            fs::rename(&self.landscape_paths.home, &recovery_dir)
                .map_err(|e| AppError::Backup(format!("mv to recovery: {e}")))?;
        }

        // 5. mkdir LANDSCAPE_HOME
        fs::create_dir_all(&self.landscape_paths.home)
            .map_err(|e| AppError::Backup(format!("mkdir HOME: {e}")))?;

        // 6. cp staging files -> HOME
        self.copy_staging_to_home(&staging_dir, &entry.scope)?;

        // 7. Cleanup staging
        let _ = fs::remove_dir_all(&staging_dir);

        // 8. Print hint
        let status_file =
            self.manager_paths.runtime_dir.join(format!("restore-{}", entry.backup_id));
        let _ = fs::create_dir_all(&self.manager_paths.runtime_dir);

        eprintln!("恢复已就绪，SSH 可安全断开。完成后执行 cat {}", status_file.display());

        Ok(())
    }

    /// Detached phase: start Landscape, health check, rollback on failure.
    pub async fn restore_detached(&self, backup_id: &str) -> Result<(), AppError> {
        let status_file = self.manager_paths.runtime_dir.join(format!("restore-{backup_id}"));

        // 1. Start Landscape (3 retries)
        let mut started = false;
        for i in 0..3 {
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            if self.service_manager.start().await.is_ok() {
                started = true;
                break;
            }
        }

        if !started {
            let msg = format!(
                "恢复结果: 失败\n备份 ID: {backup_id}\n信息: 启动失败，已保持停止状态，recovery 目录保留供人工排查\n"
            );
            let _ = fs::write(&status_file, &msg);
            return Err(AppError::Backup(
                "start failed after retries, manual intervention required".into(),
            ));
        }

        // 2. Health check
        let port = self.read_https_port();
        let healthy = self.health_check(port, Duration::from_secs(30)).await;

        if healthy {
            // 3. Success — remove recovery snapshot
            let recovery_dir = self.recovery_path(backup_id);
            let _ = fs::remove_dir_all(&recovery_dir);

            let msg = format!("恢复结果: 成功\n备份 ID: {backup_id}\n信息: 恢复完成\n");
            let _ = fs::write(&status_file, &msg);
            Ok(())
        } else {
            // 4. Failure -> rollback
            self.rollback(backup_id, &status_file).await?;
            Err(AppError::Backup("health check failed".into()))
        }
    }

    /// Rollback: stop, rm failed HOME, mv recovery -> HOME, restart.
    async fn rollback(&self, backup_id: &str, status_file: &Path) -> Result<(), AppError> {
        let _ = self.service_manager.stop().await;

        let recovery_dir = self.recovery_path(backup_id);
        let _ = fs::remove_dir_all(&self.landscape_paths.home);

        if recovery_dir.exists()
            && let Err(e) = fs::rename(&recovery_dir, &self.landscape_paths.home)
        {
            let msg = format!(
                "恢复结果: 失败\n备份 ID: {backup_id}\n信息: 回滚失败: {e}，recovery 目录保留在 {recovery_dir}\n",
                recovery_dir = recovery_dir.display()
            );
            let _ = fs::write(status_file, &msg);
            return Err(AppError::Backup(format!("rollback mv: {e}")));
        }

        // Start (best effort)
        for i in 0..3 {
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            if self.service_manager.start().await.is_ok() {
                break;
            }
        }

        let msg =
            format!("恢复结果: 失败\n备份 ID: {backup_id}\n信息: health check 超时，已自动回滚\n");
        let _ = fs::write(status_file, &msg);
        Ok(())
    }

    fn build_staging_dir(&self) -> Result<PathBuf, AppError> {
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let rand_suffix: u32 = rand::random();
        let staging_dir =
            self.manager_paths.tmp_dir.join(format!("staging-restore-{ts}-{rand_suffix:08x}"));
        fs::create_dir_all(&staging_dir)
            .map_err(|e| AppError::Backup(format!("mkdir staging: {e}")))?;
        Ok(staging_dir)
    }

    fn copy_staging_to_home(&self, staging: &Path, scope: &BackupScope) -> Result<(), AppError> {
        if *scope == BackupScope::Full {
            for entry in fs::read_dir(staging)
                .map_err(|e| AppError::Backup(format!("read staging: {e}")))?
                .flatten()
            {
                let src = entry.path();
                let name =
                    src.file_name().ok_or_else(|| AppError::Backup("invalid filename".into()))?;
                let dst = self.landscape_paths.home.join(name);
                if src.is_dir() {
                    super::copy_dir_all(&src, &dst)?;
                } else {
                    fs::copy(&src, &dst).map_err(|e| AppError::Backup(format!("cp: {e}")))?;
                }
            }
        } else {
            // Minimal restore
            fs::copy(
                staging.join("landscape-webserver"),
                self.landscape_paths.home.join("landscape-webserver"),
            )
            .map_err(|e| AppError::Backup(format!("cp binary: {e}")))?;

            if staging.join("static").exists() {
                super::copy_dir_all(&staging.join("static"), &self.landscape_paths.static_dir)?;
            }

            fs::copy(
                staging.join("landscape_init.toml"),
                self.landscape_paths.home.join("landscape_init.toml"),
            )
            .map_err(|e| AppError::Backup(format!("cp init: {e}")))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(
                    self.landscape_paths.home.join("landscape-webserver"),
                    fs::Permissions::from_mode(0o755),
                );
                let _ = fs::set_permissions(
                    self.landscape_paths.home.join("landscape_init.toml"),
                    fs::Permissions::from_mode(0o644),
                );
            }

            // Do NOT create landscape_init.lock — Landscape will re-init from init
        }

        Ok(())
    }
}
