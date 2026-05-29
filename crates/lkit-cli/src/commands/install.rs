//! `lkit install` — guided installation of Landscape.

use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use comfy_table::Table;
use comfy_table::presets::UTF8_FULL;

use lkit_app::install::InstallExecutor;
use lkit_app::source::{SourceResolver, build_release_sources, load_lkit_toml};
use lkit_client::{HttpDownloader, download::sha256_file};
use lkit_core::download::DownloadConfig;
use lkit_core::install::{
    InstallConfig, LandscapeServiceConfig, NetworkSetup, SourceSelection, WanMode, WanSetup,
};
use lkit_core::source::config::default_sources;
use lkit_core::{Artifact, ArtifactDownloader, HostInstaller, ManagerPaths, SystemTarget};

use crate::cli::InstallArgs;
use crate::progress::CliProgress;
use crate::wizard::Wizard;
use crate::wizard::nic_scan;

/// Run the install command.
///
/// Three modes:
/// - `--init-file`: non-interactive, reads existing TOML directly.
/// - TTY interactive: source selection → wizard → download → install.
/// - Non-TTY with `--source --version`: fully automatic install.
pub(crate) async fn run(
    args: InstallArgs,
    host_installer: Arc<dyn HostInstaller>,
) -> anyhow::Result<()> {
    let landscape_home = landscape_home();
    let manager_home = manager_home();

    // Pre-flight: require root (effective UID 0).
    if nix::unistd::geteuid().as_raw() != 0 {
        anyhow::bail!("需要 root 权限，请使用 sudo 运行。");
    }

    // Lock check (skip with --force)
    let lock_path = landscape_home.join("landscape_init.lock");
    if lock_path.exists() && !args.force {
        anyhow::bail!(
            "Landscape 已安装（{} 存在）。使用 --force 覆盖安装。",
            lock_path.display()
        );
    }

    let executor = InstallExecutor::new(host_installer.clone());

    // --init-file mode
    if let Some(init_file) = &args.init_file {
        let content = std::fs::read_to_string(init_file)
            .map_err(|e| anyhow::anyhow!("无法读取 {}: {e}", init_file.display()))?;

        // Validate TOML syntax
        let _: toml::Value =
            toml::from_str(&content).map_err(|e| anyhow::anyhow!("TOML 解析失败: {e}"))?;

        // Delete old lock so landscape-webserver reads the init TOML
        let lock = landscape_home.join("landscape_init.lock");
        if lock.exists() {
            tokio::fs::remove_file(&lock).await?;
        }

        let report = executor
            .execute_with_raw_toml(&content, &landscape_home)
            .await?;

        eprintln!();
        eprintln!("Landscape 安装完成！");
        eprintln!("  HOME: {}", report.home.display());
        eprintln!("  Web HTTP UI: {}", report.web_url);
        eprintln!("  Web HTTPS UI: {}", report.https_url);
        eprintln!();
        return Ok(());
    }

    // ── System detection ──
    let system_target = lkit_core::detect()?;
    let is_tty = std::io::stdin().is_terminal();

    eprintln!("  系统: {}", system_target.target_str);

    // ── Load sources ──
    let sources = if let Some(ref name) = args.source {
        // Single-source mode
        let all = load_all_sources(&manager_home)?;
        let found = all
            .into_iter()
            .find(|s| s.name == *name)
            .ok_or_else(|| anyhow::anyhow!("未知源: {name}"))?;
        vec![found]
    } else {
        load_all_sources(&manager_home)?
    };

    // ── Resolve source ──
    eprintln!("正在探测可用源...");
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let release_sources = build_release_sources(&sources, http_client.clone());
    let resolver = SourceResolver::new(release_sources);

    let (selected_source_name, resolved_tag, probe_results) = if is_tty && args.source.is_none() {
        // Interactive source selection
        let results = resolver
            .resolve(args.version.as_deref())
            .await
            .map_err(|e| anyhow::anyhow!("无法连接源: {e}\n请检查网络或使用 --init-file"))?;
        let (name, tag) = select_source_interactive(&results)?;
        (name, tag, Some(results))
    } else {
        // Non-interactive or --source specified
        let results = resolver
            .resolve(args.version.as_deref())
            .await
            .map_err(|e| anyhow::anyhow!("无法连接源: {e}\n请检查网络或使用 --init-file"))?;
        let best = &results[0];
        (
            best.source_name.clone(),
            best.resolved_tag.clone(),
            Some(results),
        )
    };

    eprintln!("  版本: {resolved_tag}  源: {selected_source_name}");

    // ── Get manifest for selected source and filter by target ──
    let selected_source = resolver
        .get_source(&selected_source_name)
        .ok_or_else(|| anyhow::anyhow!("找不到源: {selected_source_name}"))?;
    let manifest = selected_source
        .get_artifacts(&resolved_tag)
        .await
        .map_err(|e| anyhow::anyhow!("获取 manifest 失败: {e}"))?;

    let to_download: Vec<_> = manifest
        .artifacts
        .iter()
        .filter(|a| artifact_matches(a, &system_target))
        .collect();

    if to_download.is_empty() {
        anyhow::bail!("当前系统 ({}) 没有可用的制品", system_target.target_str);
    }

    // ── Wizard (TTY) or generate minimal config (non-TTY) ──
    let config = if is_tty {
        let nics = nic_scan::scan_nics();
        let single_nic = nics.len() <= 1;
        let mut wizard = Wizard::new(single_nic);
        wizard.collected.source_name = Some(selected_source_name.clone());
        wizard.collected.version = Some(resolved_tag.clone());
        wizard.collected.https_port = args.https_port;
        match wizard.run(&nics, landscape_home.clone())? {
            Some(c) => c,
            None => {
                eprintln!("安装已取消。");
                return Ok(());
            }
        }
    } else if args.source.is_some() && args.version.is_some() {
        // Non-interactive with --source --version: scan NICs
        let nics = nic_scan::scan_nics();
        let wan_nic = nics
            .iter()
            .find(|n| n.name != "lo")
            .map(|n| n.name.clone())
            .ok_or_else(|| anyhow::anyhow!("未找到可用网卡"))?;
        eprintln!("  ⚠ 非交互模式：admin 密码为空，请在安装后通过 Web UI 设置。");
        generate_minimal_config(
            &wan_nic,
            args.web_port,
            args.https_port,
            &resolved_tag,
            &landscape_home,
        )?
    } else {
        anyhow::bail!("非交互模式需要 --init-file 或 --source + --version");
    };

    // ── Download ──
    let webserver_filename = format!("landscape-webserver-{}", system_target.target_str);
    let need_download = !landscape_home.join(&webserver_filename).exists() || args.force;

    tokio::fs::create_dir_all(&landscape_home).await?;

    if need_download {
        let downloader = HttpDownloader::with_defaults()?;
        let tmp_dir = ManagerPaths::new(manager_home).tmp_dir;
        tokio::fs::create_dir_all(&tmp_dir).await?;
        eprintln!("  下载临时目录: {}", tmp_dir.display());

        // Try download with fallback (max 2 sources)
        let mut download_success = false;
        let sources_to_try =
            build_fallback_chain(&selected_source_name, probe_results.as_deref(), &resolver);

        for source in &sources_to_try {
            eprintln!("  下载中 (源: {})...", source.name());
            let result = download_artifacts(
                &to_download,
                source,
                &resolved_tag,
                &tmp_dir,
                &landscape_home,
                &downloader,
            )
            .await;
            match result {
                Ok(()) => {
                    download_success = true;
                    break;
                }
                Err(e) => {
                    eprintln!("  下载失败 ({}): {e}", source.name());
                }
            }
        }
        if !download_success {
            anyhow::bail!("所有源下载失败");
        }
    } else {
        eprintln!("  本地文件已存在，跳过下载");
    }

    // ── Chmod binaries ──
    for artifact in &to_download {
        let path = landscape_home.join(&artifact.name);
        if path.exists()
            && (artifact.name.contains("landscape-webserver")
                || artifact.name.contains("redirect_pkg_handler"))
        {
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).await?;
        }
    }

    // ── Create symlinks (systemd expects arch-less names) ──
    let webserver_src =
        landscape_home.join(format!("landscape-webserver-{}", system_target.target_str));
    let webserver_dst = landscape_home.join("landscape-webserver");
    if webserver_src.exists() && !webserver_dst.exists() {
        std::os::unix::fs::symlink(&webserver_src, &webserver_dst)?;
    }

    // ── Extract static.zip ──
    let static_zip = landscape_home.join("static.zip");
    if static_zip.exists() {
        eprintln!("  解压 static.zip...");
        let home = landscape_home.clone();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&static_zip)?;
            let mut archive = zip::ZipArchive::new(file)?;
            archive.extract(&home)?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("解压任务失败: {e}"))??;
    }

    // ── Install (TOML + systemd) ──
    // Check if service is already running (force reinstall scenario).
    let was_active = host_installer
        .is_service_active("landscape.service")
        .await
        .unwrap_or(false);

    // Delete old lock so landscape-webserver reads landscape_init.toml on startup.
    let lock = landscape_home.join("landscape_init.lock");
    if lock.exists() {
        tokio::fs::remove_file(&lock).await?;
    }

    let report = executor.execute(&config, &landscape_home).await?;

    // If service was already running, restart to pick up new init config.
    // executor.execute() calls start_service, but systemd start on a running
    // service is a no-op — a restart is needed to reload the config.
    if was_active {
        eprintln!("  检测到 landscape 服务已在运行，重启以应用新配置...");
        host_installer.restart_service("landscape.service").await?;
    }

    // ── Health check ──
    let healthy = health_check(config.landscape.https_port, 20).await;

    // ── Report ──
    let action = if was_active { "重新安装" } else { "安装完成" };
    print_report(&report, &system_target, &to_download, healthy, action);

    Ok(())
}

/// Load all available sources: CLI single-source > lkit.toml > built-in defaults.
fn load_all_sources(
    manager_home: &std::path::Path,
) -> anyhow::Result<Vec<lkit_core::SourceConfig>> {
    let user_sources =
        load_lkit_toml(manager_home).map_err(|e| anyhow::anyhow!("加载 lkit.toml 失败: {e}"))?;

    if user_sources.is_empty() {
        Ok(default_sources())
    } else {
        Ok(user_sources)
    }
}

/// Show an interactive source selection table and let the user pick.
fn select_source_interactive(
    results: &[lkit_app::source::ProbeResult],
) -> anyhow::Result<(String, String)> {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["#", "源", "版本", "延迟"]);

    for (i, r) in results.iter().enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            r.source_name.clone(),
            r.resolved_tag.clone(),
            format!("{}ms", r.latency.as_millis()),
        ]);
    }

    eprintln!();
    for line in table.to_string().lines() {
        eprintln!("  {line}");
    }
    eprintln!();

    let selection = dialoguer::Select::new()
        .with_prompt("选择安装源")
        .items(
            &results
                .iter()
                .map(|r| format!("{} ({})", r.source_name, r.resolved_tag))
                .collect::<Vec<_>>(),
        )
        .default(0)
        .interact()?;

    let chosen = &results[selection];
    Ok((chosen.source_name.clone(), chosen.resolved_tag.clone()))
}

/// Check if an artifact matches the system target.
///
/// Returns true if the artifact has no arch (arch-independent) or if
/// the arch matches the system target string.
fn artifact_matches(artifact: &Artifact, target: &SystemTarget) -> bool {
    match &artifact.arch {
        None => true,
        Some(arch) => arch == &target.target_str,
    }
}

/// Build a fallback chain of sources to try for downloading.
fn build_fallback_chain<'a>(
    primary: &str,
    probe_results: Option<&'a [lkit_app::source::ProbeResult]>,
    resolver: &'a SourceResolver,
) -> Vec<&'a Arc<dyn lkit_core::ReleaseSource>> {
    let mut names = vec![primary.to_string()];
    if let Some(results) = probe_results {
        for r in results {
            if r.source_name != primary {
                names.push(r.source_name.clone());
            }
        }
    }
    names.truncate(2); // max 2 sources
    names
        .iter()
        .filter_map(|n| resolver.get_source(n))
        .collect()
}

/// Download artifacts from a source, with SHA-256 verification.
///
/// Downloads SHASUM256sum.txt first (if present) and uses it to verify
/// all subsequent artifacts. Falls back to manifest-provided sha256,
/// then warns if no checksum source is available.
async fn download_artifacts(
    artifacts: &[&Artifact],
    source: &Arc<dyn lkit_core::ReleaseSource>,
    tag: &str,
    tmp_dir: &std::path::Path,
    dest_dir: &std::path::Path,
    downloader: &HttpDownloader,
) -> anyhow::Result<()> {
    use std::collections::HashMap;

    let progress = CliProgress::new();

    // Partition: SHASUM file(s) first, then everything else.
    let mut shasum_files = Vec::new();
    let mut other_artifacts = Vec::new();
    for artifact in artifacts {
        if artifact.name.contains("SHASUM") {
            shasum_files.push(artifact);
        } else {
            other_artifacts.push(artifact);
        }
    }

    // Checksums from SHASUM256sum.txt (filename → hex hash).
    let mut shasum_map: HashMap<String, String> = HashMap::new();

    // Phase 1: Download SHASUM file(s) and parse checksums.
    for artifact in &shasum_files {
        let url = source.artifact_url(tag, &artifact.name);
        let tmp_path = tmp_dir.join(&artifact.name);
        let final_path = dest_dir.join(&artifact.name);

        downloader
            .download(&url, &tmp_path, &DownloadConfig::default(), Some(&progress))
            .await?;

        if let Ok(content) = std::fs::read_to_string(&tmp_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some((hash, name)) = line.split_once(char::is_whitespace) {
                    shasum_map.insert(name.trim().to_string(), hash.trim().to_string());
                }
            }
        }

        tokio::fs::rename(&tmp_path, &final_path).await?;
    }

    if !shasum_map.is_empty() {
        eprintln!(
            "  已加载 SHASUM256sum.txt ({} 个 checksum)",
            shasum_map.len()
        );
    }

    // Phase 2: Download remaining artifacts with verification.
    for artifact in &other_artifacts {
        let url = source.artifact_url(tag, &artifact.name);
        let tmp_path = tmp_dir.join(&artifact.name);
        let final_path = dest_dir.join(&artifact.name);

        downloader
            .download(&url, &tmp_path, &DownloadConfig::default(), Some(&progress))
            .await?;

        // SHA-256 verification: prefer SHASUM, fall back to manifest, warn if neither.
        let expected =
            shasum_map
                .get(&artifact.name)
                .map(|s| s.as_str())
                .or(if artifact.sha256.is_empty() {
                    None
                } else {
                    Some(artifact.sha256.as_str())
                });

        match expected {
            Some(expected_hash) => {
                let hash = sha256_file(&tmp_path).await?;
                if hash != expected_hash {
                    anyhow::bail!(
                        "checksum 不匹配: {} (expected {}, got {})",
                        artifact.name,
                        expected_hash,
                        hash
                    );
                }
            }
            None => {
                eprintln!("  ⚠ {} 无 checksum，跳过校验", artifact.name);
            }
        }

        // Move from tmp to destination
        tokio::fs::rename(&tmp_path, &final_path).await?;
    }

    Ok(())
}

/// Health check — poll the HTTPS endpoint.
///
/// Returns `true` if the HTTPS endpoint is healthy within `max_attempts` × 3 seconds.
/// Returns `false` (with warning) on timeout — does not error.
async fn health_check(https_port: u16, max_attempts: u32) -> bool {
    let https_url = format!("https://127.0.0.1:{https_port}");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    for attempt in 0..max_attempts {
        match client.get(&https_url).send().await {
            Ok(resp) if resp.status().is_success() => return true,
            Ok(_) => {}
            Err(_) => {}
        }
        if attempt + 1 < max_attempts {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    false
}

/// Generate a minimal InstallConfig for non-TTY mode (single NIC, DHCP WAN, no LAN).
fn generate_minimal_config(
    wan_nic: &str,
    port: u16,
    https_port: u16,
    tag: &str,
    home: &std::path::Path,
) -> anyhow::Result<InstallConfig> {
    Ok(InstallConfig {
        network: NetworkSetup {
            wan: WanSetup {
                iface_name: wan_nic.to_string(),
                mode: WanMode::Dhcp,
            },
            lan: None,
        },
        landscape: LandscapeServiceConfig {
            web_port: port,
            https_port,
            admin_user: "root".to_string(),
            admin_pass: String::new(),
        },
        source: SourceSelection {
            source_name: None,
            version: Some(tag.to_string()),
        },
        landscape_version: tag.strip_prefix('v').unwrap_or(tag).to_string(),
        home: home.to_path_buf(),
    })
}

/// Print the installation report as a compact table.
fn print_report(
    report: &lkit_app::install::InstallReport,
    target: &SystemTarget,
    artifacts: &[&Artifact],
    healthy: bool,
    action: &str,
) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["项目", "值"]);
    table.add_row(vec!["HOME", &report.home.display().to_string()]);
    table.add_row(vec!["Web HTTP UI", &report.web_url]);
    table.add_row(vec!["Web HTTPS UI", &report.https_url]);
    table.add_row(vec!["系统", &target.target_str]);
    table.add_row(vec!["已安装组件", &artifacts.len().to_string()]);

    let status = if healthy {
        "服务已启动"
    } else {
        "健康检查超时，请手动验证: sudo systemctl status landscape"
    };
    table.add_row(vec!["状态", status]);

    eprintln!();
    eprintln!("Landscape {action}！");
    for line in table.to_string().lines() {
        eprintln!("  {line}");
    }
    eprintln!();
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
