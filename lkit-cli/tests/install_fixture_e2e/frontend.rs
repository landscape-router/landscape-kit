//! 自定义前端源(FE-01 至 FE-06)的 e2e fixture 场景:真实 systemd 部署下,
//! 自定义 HTTP 前端源的解析、应用、备份现场打包与 repair 意图驱动。
//! 场景索引见 `docs/testing/scenarios/functional/frontend.md`。

use std::io::Read;
use std::path::PathBuf;

use super::support::*;

/// 预设 config.toml:后端来源显式指向 fixture 仓库(config schema 要求
/// `repository` 段存在),`[frontend]` 声明自定义前端源。
fn frontend_preset_config(backend: &str, frontend: &str) -> String {
    format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{backend}\"\n\n[frontend]\nactive = \"custom\"\n\n[[frontend.sources]]\nid = \"custom\"\nname = \"Custom UI\"\nkind = \"http\"\nlocation = \"{frontend}\"\n"
    )
}

fn official_index() -> String {
    "<h1>Landscape fixture</h1>".into()
}

/// FE-04:install 构建版本目录后按激活的自定义前端源应用页面,原子替换
/// `releases/<version>/static/`;`static.zip` 保持官方基线;源不可达时阻断。
#[test]
fn install_applies_the_configured_custom_frontend() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("frontend-install", "healthy", 10_000);
    let frontend = RepositoryServer::start(frontend_files_for("1.0.0", "<h1>Custom frontend</h1>"));
    let preset = frontend_preset_config(&harness.repository.base_url, &frontend.base_url);
    std::fs::write(harness.config_path(), &preset).unwrap();

    assert_success(&harness.run());

    let release = harness.install_root.join("releases").join(VERSION);
    assert_eq!(
        std::fs::read_to_string(release.join("static/index.html")).unwrap(),
        "<h1>Custom frontend</h1>",
        "install must apply the custom frontend into the version directory"
    );
    assert_eq!(
        std::fs::read_to_string(harness.install_root.join("current/static/index.html")).unwrap(),
        "<h1>Custom frontend</h1>",
        "the live static directory must serve the custom frontend"
    );
    assert_eq!(
        std::fs::read(release.join("static.zip")).unwrap(),
        static_zip_for(&official_index()),
        "the version dir static.zip must stay the official baseline"
    );
    assert_eq!(
        std::fs::read(harness.config_path()).unwrap(),
        preset.as_bytes(),
        "install must not rewrite the preset config"
    );
    assert!(
        frontend
            .request_paths()
            .iter()
            .any(|path| path == "/channels/stable.json"),
        "install must resolve the custom frontend source:\n{:?}",
        frontend.request_paths()
    );

    let status = harness
        .command()
        .arg("frontend")
        .arg("status")
        .output()
        .unwrap();
    assert_success(&status);
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("custom"),
        "frontend status must name the active source:\n{stdout}"
    );
}

/// FE-04:自定义前端源不可达时,install 阻断并提示逃生路径。
#[test]
fn install_blocks_on_an_unreachable_frontend_source() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("frontend-unreachable", "healthy", 10_000);
    let preset = frontend_preset_config(&harness.repository.base_url, "http://127.0.0.1:9/");
    std::fs::write(harness.config_path(), &preset).unwrap();

    let output = harness.run();
    assert_eq!(
        output.status.code(),
        Some(1),
        "install must be blocked by the unreachable frontend source\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[frontend]") && stderr.contains("official"),
        "the block must name the escape path:\n{stderr}"
    );
    assert!(
        !harness.install_root.join("releases").join(VERSION).exists(),
        "the failed install must not leave a version directory"
    );
}

/// FE-06:激活自定义前端源时 `repair static` 重新拉取自定义前端;
/// `--official` 无条件恢复官方页面、更新 state 身份并刷新版本目录 `static.zip`。
#[test]
fn repair_static_restores_custom_frontend_then_official_flag() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("frontend-repair", "healthy", 10_000);
    let frontend = RepositoryServer::start(frontend_files_for("1.0.0", "<h1>Custom frontend</h1>"));
    std::fs::write(
        harness.config_path(),
        frontend_preset_config(&harness.repository.base_url, &frontend.base_url),
    )
    .unwrap();
    assert_success(&harness.run());

    let index = harness.install_root.join("current/static/index.html");
    std::fs::write(&index, "<h1>tampered</h1>").unwrap();

    let repair = harness
        .command()
        .arg("repair")
        .arg("static")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&repair);
    assert_eq!(
        std::fs::read_to_string(&index).unwrap(),
        "<h1>Custom frontend</h1>",
        "repair static must restore the active custom frontend"
    );

    std::fs::write(&index, "<h1>tampered again</h1>").unwrap();
    let official = harness
        .command()
        .arg("repair")
        .arg("static")
        .arg("--official")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&official);
    assert_eq!(
        std::fs::read_to_string(&index).unwrap(),
        official_index(),
        "--official must unconditionally restore the official pages"
    );
    assert_eq!(
        std::fs::read(harness.install_root.join("releases/1.2.3/static.zip")).unwrap(),
        static_zip_for(&official_index()),
        "the official path must refresh the version dir static.zip"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    let (sha, size) = sha256(&static_zip_for(&official_index()));
    assert_eq!(
        state["assets"]["static_archive"]["sha256"]
            .as_str()
            .unwrap(),
        sha,
        "the official path must update the state static identity"
    );
    assert_eq!(
        state["assets"]["static_archive"]["size"].as_u64().unwrap(),
        size
    );
}

/// FE-05:备份从 `current/static/` 现场打包(归档内 `static.zip` 含自定义前端
/// 内容);恢复不校验身份,恢复内容即备份快照。
#[test]
fn backup_packs_live_static_and_restore_returns_snapshot() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("frontend-backup", "healthy", 10_000);
    let frontend = RepositoryServer::start(frontend_files_for("1.0.0", "<h1>Custom frontend</h1>"));
    std::fs::write(
        harness.config_path(),
        frontend_preset_config(&harness.repository.base_url, &frontend.base_url),
    )
    .unwrap();
    assert_success(&harness.run());

    let created = harness
        .command()
        .arg("backup")
        .arg("create")
        .arg("--remark")
        .arg("frontend snapshot")
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&created);

    let backups: Vec<PathBuf> = std::fs::read_dir(harness.backups_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("lkb"))
        .collect();
    assert_eq!(backups.len(), 1, "exactly one manual backup must exist");

    let file = std::fs::File::open(&backups[0]).unwrap();
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut packed_static_zip = None::<Vec<u8>>;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == std::path::Path::new("static.zip") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            packed_static_zip = Some(bytes);
        }
    }
    let packed_static_zip = packed_static_zip.expect("the .lkb must contain static.zip");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(packed_static_zip)).unwrap();
    let mut packed_index = String::new();
    zip.by_name("static/index.html")
        .unwrap()
        .read_to_string(&mut packed_index)
        .unwrap();
    assert_eq!(
        packed_index, "<h1>Custom frontend</h1>",
        "the backup must pack the live custom frontend"
    );

    std::fs::write(
        harness.install_root.join("current/static/index.html"),
        "<h1>hacked</h1>",
    )
    .unwrap();
    let backup_id = backups[0]
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let restored = harness
        .command()
        .arg("restore")
        .arg("--backup")
        .arg(&backup_id)
        .arg("--yes")
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&restored);
    assert_eq!(
        std::fs::read_to_string(harness.install_root.join("current/static/index.html")).unwrap(),
        "<h1>Custom frontend</h1>",
        "restore must return the snapshot content without identity checks"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_success(&active);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}
