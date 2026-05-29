//! `lkit install` — guided installation of Landscape.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lkit_app::install::InstallExecutor;
use lkit_core::HostInstaller;

use crate::cli::InstallArgs;
use crate::wizard::nic_scan;
use crate::wizard::Wizard;

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

    let config = if let Some(init_file) = &args.init_file {
        // Non-interactive: read existing TOML
        read_init_file(init_file)?
    } else {
        // Interactive: run wizard
        let nics = nic_scan::scan_nics();
        let single_nic = nics.len() <= 1;
        let mut wizard = Wizard::new(single_nic);

        match wizard.run(&nics)? {
            Some(config) => config,
            None => {
                eprintln!("安装已取消。");
                return Ok(());
            }
        }
    };

    // Execute installation
    let executor = InstallExecutor::new(host_installer);
    let report = executor.execute(&config, &landscape_home).await?;

    eprintln!();
    eprintln!("Landscape 安装完成！");
    eprintln!("  HOME: {}", report.home.display());
    eprintln!("  Web UI: {}", report.web_url);
    eprintln!();

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

/// Read and parse a landscape_init.toml file, converting to InstallConfig.
///
/// Note: This reads the raw TOML for `--init-file` mode. The TOML is passed
/// directly to the executor rather than being converted through InstallConfig.
/// For now, we extract the version field and construct a minimal InstallConfig
/// from the TOML content.
fn read_init_file(path: &Path) -> anyhow::Result<lkit_core::InstallConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("无法读取 {}: {e}", path.display()))?;

    // Validate TOML syntax
    let _parsed: toml::Value = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("TOML 解析失败: {e}"))?;

    // Extract version for InstallConfig (the executor will generate its own TOML)
    let version = _parsed
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .to_string();

    // Extract basic fields to construct InstallConfig
    // For --init-file mode, we create a minimal config and let the executor
    // generate the TOML from it. The actual TOML content from the file is
    // what should be written — but our executor generates its own TOML.
    //
    // For V1, --init-file validates the TOML and extracts the version.
    // A future improvement would be to pass the raw TOML directly to the executor.
    let admin_user = _parsed
        .get("config")
        .and_then(|c| c.get("auth"))
        .and_then(|a| a.get("admin_user"))
        .and_then(|v| v.as_str())
        .unwrap_or("root")
        .to_string();

    let admin_pass = _parsed
        .get("config")
        .and_then(|c| c.get("auth"))
        .and_then(|a| a.get("admin_pass"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let web_port = _parsed
        .get("config")
        .and_then(|c| c.get("web"))
        .and_then(|w| w.get("port"))
        .and_then(|v| v.as_integer())
        .unwrap_or(6300) as u16;

    Ok(lkit_core::InstallConfig {
        network: lkit_core::NetworkSetup {
            wan: lkit_core::WanSetup {
                iface_name: "eth0".to_string(),
                mode: lkit_core::WanMode::Dhcp,
            },
            lan: None,
        },
        landscape: lkit_core::LandscapeServiceConfig {
            web_port,
            admin_user,
            admin_pass,
        },
        source: lkit_core::SourceSelection {
            source_name: None,
            version: None,
        },
        landscape_version: version,
    })
}
