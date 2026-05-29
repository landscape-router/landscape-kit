//! `lkit install` — guided installation of Landscape.

use std::path::PathBuf;
use std::sync::Arc;

use lkit_app::install::InstallExecutor;
use lkit_core::HostInstaller;

use crate::cli::InstallArgs;
use crate::wizard::Wizard;
use crate::wizard::nic_scan;

/// Run the install command.
///
/// Two modes:
/// - Interactive: runs the wizard to collect config from the user.
/// - Non-interactive (`--init-file`): reads an existing `landscape_init.toml`.
pub(crate) async fn run(
    args: InstallArgs,
    host_installer: Arc<dyn HostInstaller>,
) -> anyhow::Result<()> {
    let landscape_home = landscape_home();

    // Check if already installed (landscape_init.lock exists)
    let lock_path = landscape_home.join("landscape_init.lock");
    if lock_path.exists() {
        anyhow::bail!(
            "Landscape 已安装（{} 存在）。如需重新安装，请先删除该文件。",
            lock_path.display()
        );
    }

    let executor = InstallExecutor::new(host_installer);

    if let Some(init_file) = &args.init_file {
        // Non-interactive: read existing TOML and write it directly
        let content = std::fs::read_to_string(init_file)
            .map_err(|e| anyhow::anyhow!("无法读取 {}: {e}", init_file.display()))?;

        // Validate TOML syntax
        let _: toml::Value =
            toml::from_str(&content).map_err(|e| anyhow::anyhow!("TOML 解析失败: {e}"))?;

        let report = executor
            .execute_with_raw_toml(&content, &landscape_home)
            .await?;

        eprintln!();
        eprintln!("Landscape 安装完成！");
        eprintln!("  HOME: {}", report.home.display());
        eprintln!("  Web UI: {}", report.web_url);
        eprintln!();
    } else {
        // Interactive: run wizard
        let nics = nic_scan::scan_nics();
        let single_nic = nics.len() <= 1;
        let mut wizard = Wizard::new(single_nic);

        let config = match wizard.run(&nics)? {
            Some(config) => config,
            None => {
                eprintln!("安装已取消。");
                return Ok(());
            }
        };

        let report = executor.execute(&config, &landscape_home).await?;

        eprintln!();
        eprintln!("Landscape 安装完成！");
        eprintln!("  HOME: {}", report.home.display());
        eprintln!("  Web UI: {}", report.web_url);
        eprintln!();
    }

    Ok(())
}

/// Determine Landscape HOME path.
fn landscape_home() -> PathBuf {
    std::env::var("LANDSCAPE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            PathBuf::from(home).join(".landscape-router")
        })
}
