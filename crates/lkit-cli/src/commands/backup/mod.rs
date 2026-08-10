use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::deployment::plan;
use crate::deployment::root::InstallRoot;

mod create;
mod delete;
mod inspect;
mod list;

use self::create::run_create;
use self::delete::run_delete;
use self::inspect::{run_show, run_verify};
use self::list::run_list;

pub(crate) use create::create_manual_backup;
pub(crate) use delete::delete_backup;
pub(crate) use list::{BackupListCheck, list_backups_with};

#[derive(Debug, Args)]
pub struct Backup {
    #[command(subcommand)]
    pub action: BackupAction,
}

#[derive(Debug, Subcommand)]
pub enum BackupAction {
    /// 创建手工 minimal 备份
    Create(BackupCreate),
    /// 列出安装根目录下的备份
    List(BackupList),
    /// 展示备份 metadata 与边界
    Show(BackupShow),
    /// 完整校验备份
    Verify(BackupVerify),
    /// 删除安装根目录下的备份
    Delete(BackupDelete),
}

#[derive(Debug, Args)]
pub struct BackupCreate {
    /// 最多 256 个字符的单行说明
    #[arg(long, value_name = "TEXT")]
    pub remark: Option<String>,
    /// 将备份原子写入指定新文件(不得已存在)
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BackupList {
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BackupShow {
    /// 安装根目录 `backups/` 下的备份 ID
    #[arg(long, value_name = "ID", conflicts_with = "file")]
    pub backup: Option<String>,
    /// 外部复制的 `.lkb` 文件路径
    #[arg(long, value_name = "PATH", conflicts_with = "backup")]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BackupVerify {
    /// 安装根目录 `backups/` 下的备份 ID
    #[arg(long, value_name = "ID", conflicts_with = "file")]
    pub backup: Option<String>,
    /// 外部复制的 `.lkb` 文件路径
    #[arg(long, value_name = "PATH", conflicts_with = "backup")]
    pub file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BackupDelete {
    /// 安装根目录 `backups/` 下的备份 ID
    #[arg(long, value_name = "ID")]
    pub backup: String,
    /// 非交互模式确认删除
    #[arg(long)]
    pub yes: bool,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
}

pub async fn run(args: &Backup) -> ExitCode {
    match &args.action {
        BackupAction::Create(args) => run_create(args).await,
        BackupAction::List(args) => run_list(args),
        BackupAction::Show(args) => run_show(args),
        BackupAction::Verify(args) => run_verify(args),
        BackupAction::Delete(args) => run_delete(args),
    }
}

fn resolve_root(install_dir: Option<&Path>) -> Result<InstallRoot, plan::InstallError> {
    let install_root = plan::select_install_root(
        install_dir,
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    crate::deployment::root::normalize_install_root(&install_root)
}

pub(crate) fn architecture_key(
    architecture: crate::backup::lkb::BackupArchitecture,
) -> &'static str {
    match architecture {
        crate::backup::lkb::BackupArchitecture::X86_64 => "x86_64",
        crate::backup::lkb::BackupArchitecture::Aarch64 => "aarch64",
    }
}

pub(crate) fn scope_key(scope: crate::backup::lkb::BackupScope) -> &'static str {
    match scope {
        crate::backup::lkb::BackupScope::Minimal => "minimal",
    }
}

fn exit_code(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) => ExitCode::from(2),
        _ => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lkit-backup-cmd-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[cfg(feature = "test-support")]
    use crate::deployment::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, ServiceState,
        StateArchitecture, StateServiceManager, WebserverAsset,
    };

    #[cfg(feature = "test-support")]
    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (hex, bytes.len() as u64)
    }

    /// 构造一个初始化完成、`none` service manager 的假安装现场,
    /// 与 `backup create` 读取的路径保持一致。
    #[cfg(feature = "test-support")]
    fn fake_install(dir: &std::path::Path) -> InstallState {
        use std::os::unix::fs::symlink;
        let version = "1.2.3";
        let release = dir.join("releases").join(version);
        std::fs::create_dir_all(release.join("static/assets")).unwrap();
        let payload = b"webserver payload 1.2.3";
        let zip = b"zip payload 1.2.3";
        std::fs::write(release.join("landscape-webserver"), payload).unwrap();
        std::fs::write(release.join("static.zip"), zip).unwrap();
        std::fs::write(release.join("static/index.html"), "<h1>1.2.3</h1>").unwrap();
        symlink(format!("releases/{version}"), dir.join("current")).unwrap();
        std::fs::create_dir_all(dir.join("data/geo_tmp/ip")).unwrap();
        std::fs::write(dir.join("data/geo_tmp/ip/geo.dat"), b"geo").unwrap();
        std::fs::write(dir.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(dir.join("data/landscape.toml"), b"").unwrap();
        let (webserver_sha, webserver_size) = sha256_bytes(payload);
        let (static_sha, static_size) = sha256_bytes(zip);
        InstallState {
            schema_version: 1,
            layout_version: 1,
            install_root: dir.display().to_string(),
            canonical_install_root: dir.display().to_string(),
            active_version: version.into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: webserver_sha,
                    size: webserver_size,
                },
                static_archive: ArchiveAsset {
                    sha256: static_sha,
                    size: static_size,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            },
            service: ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        }
    }

    #[cfg(feature = "test-support")]
    fn runtime_file(dir: &std::path::Path, export_base: &str) -> std::path::PathBuf {
        use std::os::unix::fs::MetadataExt;
        let runtime = dir.join("runtime.json");
        let host = dir.join("host");
        let content = serde_json::json!({
            "schema_version": 1,
            "allow_non_root": true,
            "preflight": "skip",
            "execution": "inline",
            "managed_uid": std::fs::metadata(dir).unwrap().uid(),
            "os_release_path": "/etc/os-release",
            "systemd": {
                "systemctl": "/bin/false",
                "system_unit_dir": host.join("units"),
                "run_systemd_dir": host.join("run/systemd/system"),
                "pid1_is_systemd": false,
                "resolv_conf": host.join("resolv.conf"),
            },
            "health": {
                "base_url": export_base,
                "dns_tcp_port": 1053,
                "dns_udp_port": 1053,
                "http_port": 6300,
                "https_port": 6443,
                "startup_timeout_ms": 1000,
                "stable_duration_ms": 1000,
            },
            "export_base_url": export_base,
        });
        std::fs::write(&runtime, serde_json::to_vec_pretty(&content).unwrap()).unwrap();
        runtime
    }

    #[cfg(feature = "test-support")]
    fn export_ok_server(version: &str) -> crate::release::repository::test_server::TestServer {
        use crate::release::repository::test_server::{TestResponse, TestServer};
        let version = version.to_string();
        TestServer::start(move |path| {
            if path == crate::backup::export::EXPORT_PATH {
                TestResponse::ok(
                    format!(
                        r#"{{"data":{{"filename":"landscape_init_v{version}.toml","version":"{version}","content":"version = \"{version}\"\n"}}}}"#
                    )
                    .into_bytes(),
                )
            } else {
                TestResponse::status(404, "Not Found", Vec::new())
            }
        })
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn create_writes_manual_backup_without_any_service_manager() {
        let temp = temp_dir("create-ok");
        let dir = temp.join("install");
        std::fs::create_dir_all(&dir).unwrap();
        let state = fake_install(&dir);
        crate::deployment::state::write_state(
            &crate::deployment::root::InstallRoot {
                install_root: dir.clone(),
                canonical: dir.clone(),
            },
            &state,
        )
        .unwrap();
        std::fs::write(dir.join("data/landscape_api_token"), b"tok\n").unwrap();
        std::fs::set_permissions(
            dir.join("data/landscape_api_token"),
            std::fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        let server = export_ok_server("1.2.3");
        let runtime = runtime_file(&temp, &server.base);
        let args = BackupCreate {
            remark: Some("manual create".into()),
            output: None,
            install_dir: Some(dir.clone()),
            test_runtime: Some(runtime),
        };
        assert_eq!(run_create(&args).await, ExitCode::SUCCESS);
        let backups = dir.join("backups");
        let mut lkb = std::fs::read_dir(&backups)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"));
        let lkb_path = lkb.next().expect("backup create must write a .lkb file");
        assert!(lkb.next().is_none(), "only one .lkb must be created");
        let bytes = std::fs::read(lkb_path.path()).unwrap();
        let metadata = crate::backup::lkb::verify_lkb(&bytes).unwrap();
        assert!(!metadata.auto, "manual backup must record auto: false");
        assert_eq!(metadata.remark, "manual create");
        assert_eq!(metadata.landscape_version, "1.2.3");
        let extracted = temp.join("extracted");
        crate::backup::lkb::extract_lkb(&bytes, &extracted).unwrap();
        assert_eq!(
            std::fs::read_to_string(extracted.join("landscape_init.toml")).unwrap(),
            "version = \"1.2.3\"\n",
            "the archive must carry the exported config, not the seed file"
        );
        assert_eq!(
            std::fs::read(extracted.join("landscape-webserver")).unwrap(),
            b"webserver payload 1.2.3"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn create_export_failure_leaves_no_final_file() {
        use crate::release::repository::test_server::{TestResponse, TestServer};
        let temp = temp_dir("create-fail");
        let dir = temp.join("install");
        std::fs::create_dir_all(&dir).unwrap();
        let state = fake_install(&dir);
        crate::deployment::state::write_state(
            &crate::deployment::root::InstallRoot {
                install_root: dir.clone(),
                canonical: dir.clone(),
            },
            &state,
        )
        .unwrap();
        std::fs::write(dir.join("data/landscape_api_token"), b"tok\n").unwrap();
        std::fs::set_permissions(
            dir.join("data/landscape_api_token"),
            std::fs::Permissions::from_mode(0o400),
        )
        .unwrap();
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        let runtime = runtime_file(&temp, &server.base);
        let args = BackupCreate {
            remark: None,
            output: None,
            install_dir: Some(dir.clone()),
            test_runtime: Some(runtime),
        };
        assert_eq!(run_create(&args).await, ExitCode::FAILURE);
        let backups = dir.join("backups");
        assert!(
            !backups.exists() || std::fs::read_dir(&backups).unwrap().count() == 0,
            "export failure must not leave any backup files behind"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
