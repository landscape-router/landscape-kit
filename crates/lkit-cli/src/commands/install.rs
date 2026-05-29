//! `lkit install` — guided installation of Landscape.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lkit_app::install::InstallExecutor;
use lkit_app::source::{SourceResolver, build_release_sources};
use lkit_client::{HttpDownloader, download::sha256_file};
use lkit_core::download::DownloadConfig;
use lkit_core::source::config::default_sources;
use lkit_core::{ArtifactDownloader, HostInstaller, ManagerPaths};

use crate::cli::InstallArgs;
use crate::progress::CliProgress;
use crate::wizard::Wizard;
use crate::wizard::nic_scan;

/// Run the install command.
///
/// Two modes:
/// - Interactive: source probe → wizard → download (if needed) → install.
/// - Non-interactive (`--init-file`): reads an existing `landscape_init.toml`.
pub(crate) async fn run(
    args: InstallArgs,
    host_installer: Arc<dyn HostInstaller>,
) -> anyhow::Result<()> {
    let landscape_home = landscape_home();

    // Pre-flight: require root (effective UID 0).
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("需要 root 权限，请使用 sudo 运行。");
    }

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
        // ── Step 1: Multi-source probe to resolve version + manifest ──
        eprintln!("正在探测可用源...");
        let sources = default_sources();
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        let release_sources = build_release_sources(&sources, http_client.clone());
        let resolver = SourceResolver::new(release_sources);

        let results = resolver
            .resolve(None)
            .await
            .map_err(|e| anyhow::anyhow!("无法连接官方源: {e}\n请检查网络或使用 --init-file"))?;

        let best = &results[0];
        let resolved_tag = best.resolved_tag.clone();

        eprintln!("  版本: {resolved_tag}  源: {}  延迟: {:?}", best.source_name, best.latency);

        // ── Step 2: Wizard ──
        let nics = nic_scan::scan_nics();
        let single_nic = nics.len() <= 1;
        let mut wizard = Wizard::new(single_nic);
        wizard.collected.version = Some(resolved_tag.clone());

        let config = match wizard.run(&nics, landscape_home.clone())? {
            Some(config) => config,
            None => {
                eprintln!("安装已取消。");
                return Ok(());
            }
        };

        // ── Step 3: Download if needed ──
        let binary_path = landscape_home.join("landscape-webserver");
        let need_download = !binary_path.exists();

        if need_download {
            let arch = std::env::consts::ARCH;
            let artifacts = best.manifest.artifacts_for_arch(arch);

            if artifacts.is_empty() {
                anyhow::bail!("当前架构 ({arch}) 没有可用的制品");
            }

            let downloader = HttpDownloader::with_defaults()?;
            let manager_home = manager_home();
            let tmp_dir = ManagerPaths::new(manager_home).tmp_dir;
            tokio::fs::create_dir_all(&tmp_dir).await?;

            let progress = CliProgress::new();

            for artifact in &artifacts {
                let source = resolver
                    .get_source(&best.source_name)
                    .ok_or_else(|| anyhow::anyhow!("找不到源: {}", best.source_name))?;
                let url = source.artifact_url(&resolved_tag, &artifact.name);
                let dest = tmp_dir.join(&artifact.name);

                downloader
                    .download(
                        &url,
                        &dest,
                        &DownloadConfig::default(),
                        Some(&progress),
                    )
                    .await?;

                // SHA-256 verification
                if !artifact.sha256.is_empty() {
                    let hash = sha256_file(&dest).await?;
                    if hash != artifact.sha256 {
                        anyhow::bail!(
                            "checksum 不匹配: {} (expected {}, got {})",
                            artifact.name, artifact.sha256, hash
                        );
                    }
                }

                // Move from tmp to landscape_home
                let final_dest = landscape_home.join(&artifact.name);
                tokio::fs::rename(&dest, &final_dest).await?;

                // Binary needs execute permission
                if artifact.name.contains("landscape-webserver") {
                    tokio::fs::set_permissions(
                        &final_dest,
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .await?;
                }
            }

            // Extract static.zip if present
            let static_zip = landscape_home.join("static.zip");
            if static_zip.exists() {
                let static_dir = landscape_home.join("static");
                tokio::fs::create_dir_all(&static_dir).await?;
                // TODO: extract zip when needed
            }
        } else {
            eprintln!("  本地 binary 已存在，跳过下载");
        }

        // ── Step 4: Generate TOML + systemd ──
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

/// Determine manager HOME path.
fn manager_home() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".landscape-kit"))
        .unwrap_or_else(|_| PathBuf::from("/root/.landscape-kit"))
}
