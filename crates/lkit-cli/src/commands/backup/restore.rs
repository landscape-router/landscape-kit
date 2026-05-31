//! `lkit backup restore` and hidden `lkit backup do-restore`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::messages::CliMessages;

/// Run `lkit backup restore <id_or_path>`.
///
/// Performs pre-checks, creates a recovery snapshot, then dispatches the
/// actual restore via `systemd-run` for SSH disconnection protection.
pub(crate) async fn run(id_or_path: &str, state: &AppState) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entry = usecase.resolve(id_or_path).await.map_err(|e| anyhow::anyhow!("{}", e))?;

    // Confirm with user.
    let mut confirm_params = HashMap::new();
    confirm_params.insert("id", entry.id.as_str());
    let prompt = CliMessages::format("backup.confirm_delete", &confirm_params);
    eprintln!("{}", prompt);
    eprintln!("将启动 systemd 后台任务执行恢复。确认? [y/N] ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        eprintln!("已取消");
        return Ok(());
    }

    // Create recovery snapshot.
    let ts = lkit_app::backup::builder::timestamp();
    let recovery_dir = state.manager_paths.runtime_dir.join(format!("recovery-{ts}"));
    std::fs::create_dir_all(&recovery_dir)?;
    create_recovery_snapshot(state, &recovery_dir)?;
    eprintln!("{}", CliMessages::format("backup.recovery_snapshot_created", &HashMap::new()));

    // Dispatch to systemd-run.
    let self_path = std::env::current_exe()?;
    let status = Command::new("systemd-run")
        .args([
            "--unit=lkit-restore",
            "--same-dir",
            "--collect",
            "--service-type=oneshot",
            "--",
            &self_path.to_string_lossy(),
            "backup",
            "do-restore",
            &entry.id,
            "--recovery-dir",
            &recovery_dir.to_string_lossy(),
        ])
        .spawn();

    match status {
        Ok(_) => {
            println!("{}", CliMessages::format("backup.restore_started", &HashMap::new()));
        }
        Err(e) => {
            anyhow::bail!("无法启动 systemd-run: {e}。请手动执行恢复。");
        }
    }

    Ok(())
}

/// Run `lkit backup do-restore <id> --recovery-dir <path>`.
///
/// This is the hidden subcommand that performs the actual restore under
/// systemd-run protection. It is NOT meant to be called directly by users.
pub(crate) async fn run_do_restore(
    id: &str,
    recovery_dir: &Path,
    state: &AppState,
) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entry = usecase.resolve(id).await?;

    match usecase.restore(&entry, recovery_dir).await {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(recovery_dir);
            println!("{}", CliMessages::format("backup.restored", &HashMap::new()));
            Ok(())
        }
        Err(e) => {
            eprintln!(
                "{}: {e}",
                CliMessages::format("backup.restore_failed_rolled_back", &HashMap::new())
            );
            std::process::exit(1);
        }
    }
}

/// Create a recovery snapshot of the current Landscape installation.
fn create_recovery_snapshot(state: &AppState, recovery_dir: &Path) -> Result<(), anyhow::Error> {
    let home = &state.landscape_paths.home;

    // Copy binary.
    let bin_src = home.join("landscape-webserver");
    if bin_src.exists() {
        let bin_dst = recovery_dir.join("landscape-webserver");
        std::fs::copy(&bin_src, &bin_dst)?;
    }

    // Copy static assets.
    let static_src = &state.landscape_paths.static_dir;
    if static_src.exists() {
        let static_dst = recovery_dir.join("static");
        copy_dir_recursive(static_src, &static_dst)?;
    }

    // Copy config.
    let config_src = home.join("landscape_init.toml");
    if config_src.exists() {
        let config_dst = recovery_dir.join("landscape_init.toml");
        std::fs::copy(&config_src, &config_dst)?;
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), anyhow::Error> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
