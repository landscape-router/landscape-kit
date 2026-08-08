use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::backup::lkb::{
    BackupMetadata, LKB_METADATA_CAPACITY, backup_id_format_ok, validate_remark, verify_lkb,
};
use crate::deployment::root::InstallRoot;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, state, transaction};
use crate::release::artifacts::WEBSERVER_BINARY;
use crate::workflows::restore::validate_backup_file;

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

pub async fn run(args: &Backup) -> ExitCode {
    match &args.action {
        BackupAction::Create(args) => run_create(args).await,
        BackupAction::List(args) => run_list(args),
        BackupAction::Show(args) => run_show(args),
        BackupAction::Verify(args) => run_verify(args),
    }
}

async fn run_create(args: &BackupCreate) -> ExitCode {
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT)
        );
        return ExitCode::FAILURE;
    }
    let root = match resolve_root(args.install_dir.as_deref()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let remark = match resolve_remark(&args.remark) {
        Ok(remark) => remark,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock(&root) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let unfinished = match transaction::find_unfinished(&root) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    if let Some(transaction) = unfinished
        && let Err(error) =
            transaction::recover_interrupted(&root, &transaction, &runtime.systemd, &health).await
    {
        eprintln!("backup: {error}");
        return exit_code(&error);
    }
    let Some(installed) = (match state::load_state(&root) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    }) else {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION)
        );
        return ExitCode::from(2);
    };
    let result = (async {
        crate::workflows::install::check_initialization(&root, &installed)?;
        crate::workflows::install::verify_current_backend(&root, &installed)?;
        let token = crate::backup::export::read_api_token(
            &root.canonical.join("data/landscape_api_token"),
            runtime.managed_uid,
        )?;
        let exported =
            crate::backup::export::export_config(&runtime.export_base_url, &token).await?;
        if exported.version != installed.active_version {
            return Err(plan::InstallError::ExportFailed(format!(
                "exported version {} does not match the running version {}",
                exported.version, installed.active_version
            )));
        }
        let version = crate::workflows::install::parse_stable_version(&installed.active_version)
            .map_err(|error| {
                plan::InstallError::CorruptedState(format!("invalid active version: {error}"))
            })?;
        let architecture = match installed.assets.webserver.architecture {
            crate::deployment::state::StateArchitecture::X86_64 => "x86_64",
            crate::deployment::state::StateArchitecture::Aarch64 => "aarch64",
        };
        let webserver = root
            .canonical
            .join("releases")
            .join(&installed.active_version)
            .join(WEBSERVER_BINARY);
        let static_dir = root.canonical.join("current/static");
        let static_archive = root
            .canonical
            .join("releases")
            .join(&installed.active_version)
            .join("static.zip");
        let geo_tmp = root.canonical.join("data/geo_tmp");
        let backup_ref = crate::backup::lkb::create_backup(
            &root.canonical.join("backups"),
            &version,
            architecture,
            &webserver,
            &exported.content,
            &static_dir,
            &static_archive,
            &geo_tmp,
            &remark,
            false,
        )?;
        if let Some(output) = &args.output {
            let final_path = root.canonical.join(&backup_ref.path);
            copy_backup_to_output(&final_path, output)?;
        }
        let metadata = verify_lkb(
            &std::fs::read(root.canonical.join(&backup_ref.path))
                .map_err(plan::InstallError::Io)?,
        )?;
        Ok(metadata)
    })
    .await;
    match result {
        Ok(metadata) => {
            let path = if let Some(output) = &args.output {
                output.display().to_string()
            } else {
                root.canonical
                    .join("backups")
                    .join(format!("{}.lkb", metadata.backup_id))
                    .display()
                    .to_string()
            };
            println!(
                "backup: {}",
                crate::tr!(
                    crate::keys::BACKUP_CREATED,
                    backup_id = metadata.backup_id,
                    version = metadata.landscape_version,
                    path = path
                )
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("backup: {error}");
            exit_code(&error)
        }
    }
}

fn run_list(args: &BackupList) -> ExitCode {
    let root = match resolve_root(args.install_dir.as_deref()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let backups_dir = root.canonical.join("backups");
    let rows = match list_backups(&root) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    for (parsed, path) in &rows {
        match parsed {
            Some(metadata) => {
                println!(
                    "{} {} {} {} auto={} scope={} remark={} status=valid",
                    metadata.backup_id,
                    metadata.created_at,
                    metadata.landscape_version,
                    architecture_key(metadata.architecture),
                    metadata.auto,
                    scope_key(metadata.scope),
                    metadata.remark,
                );
            }
            None => {
                println!(
                    "{} status=invalid",
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .trim_end_matches(".lkb")
                );
            }
        }
    }
    let invalid = rows
        .iter()
        .filter(|(metadata, _)| metadata.is_none())
        .count();
    if invalid > 0 {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_LIST_INVALID, count = invalid)
        );
        return ExitCode::FAILURE;
    }
    if rows.is_empty() {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_NONE_FOUND, dir = backups_dir.display())
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// 读取安装根目录 `backups/` 下的 `.lkb` 文件并完整校验,按创建时间降序排列。
/// 目录缺失时返回空列表;校验失败的条目 metadata 为 `None`(视为损坏)。
/// 临时目录或解包写入失败等环境错误直接返回,不得把全部备份误报为损坏。
pub(crate) fn list_backups(
    root: &InstallRoot,
) -> Result<Vec<(Option<BackupMetadata>, PathBuf)>, plan::InstallError> {
    let backups_dir = root.canonical.join("backups");
    let entries = match std::fs::read_dir(&backups_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(plan::InstallError::Io(error)),
    };
    let mut rows: Vec<(Option<BackupMetadata>, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("lkb") {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.file_type().is_file() || validate_backup_file(&path).is_err() {
            rows.push((None, path));
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let parsed = match verify_lkb(&bytes) {
            Ok(parsed) => parsed,
            Err(_) => {
                rows.push((None, path));
                continue;
            }
        };
        // 内容完整性校验与 verify 相同:归档必须包含全部必需条目。
        let verify_dir =
            std::env::temp_dir().join(format!("lkit-backup-list-{}", uuid::Uuid::now_v7()));
        let content = crate::backup::lkb::create_secure_dir(&verify_dir, 0o700)
            .and_then(|()| crate::backup::lkb::extract_lkb(&bytes, &verify_dir));
        match content {
            Ok(_) => rows.push((Some(parsed), path)),
            Err(plan::InstallError::InvalidBackup(_)) => {
                rows.push((None, path));
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&verify_dir);
                return Err(error);
            }
        }
        let _ = std::fs::remove_dir_all(&verify_dir);
    }
    rows.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(a), Some(b)) => b.created_at.cmp(&a.created_at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Ok(rows)
}

fn run_show(args: &BackupShow) -> ExitCode {
    let (bytes, label) =
        match resolve_backup_bytes(&args.backup, &args.file, args.install_dir.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("backup: {error}");
                return exit_code(&error);
            }
        };
    let metadata = match verify_lkb(&bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("backup: {error}");
            return ExitCode::FAILURE;
        }
    };
    let metadata_len = metadata_bytes_len(&bytes).unwrap_or(0);
    println!("{label}");
    println!("backup_id: {}", metadata.backup_id);
    println!("created_at: {}", metadata.created_at);
    println!("landscape_version: {}", metadata.landscape_version);
    println!("lkit_version: {}", metadata.lkit_version);
    println!("architecture: {}", architecture_key(metadata.architecture));
    println!("hostname: {}", metadata.hostname);
    println!("remark: {}", metadata.remark);
    println!("auto: {}", metadata.auto);
    println!("scope: {}", scope_key(metadata.scope));
    println!(
        "contents: binary={} static={} static_archive={} init_config={} geo_cache={}",
        metadata.contents.binary,
        metadata.contents.static_,
        metadata.contents.static_archive,
        metadata.contents.init_config,
        metadata.contents.geo_cache,
    );
    println!("header_bytes: 32");
    println!("metadata_bytes: {metadata_len}");
    println!(
        "archive_bytes: {}",
        bytes.len().saturating_sub(LKB_METADATA_CAPACITY)
    );
    ExitCode::SUCCESS
}

fn run_verify(args: &BackupVerify) -> ExitCode {
    let (bytes, label) =
        match resolve_backup_bytes(&args.backup, &args.file, args.install_dir.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("backup: {error}");
                return exit_code(&error);
            }
        };
    let metadata = match verify_lkb(&bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("backup: {error}");
            return ExitCode::FAILURE;
        }
    };
    let verify_dir =
        std::env::temp_dir().join(format!("lkit-backup-verify-{}", uuid::Uuid::now_v7()));
    if let Err(error) = crate::backup::lkb::create_secure_dir(&verify_dir, 0o700) {
        eprintln!("backup: {error}");
        return exit_code(&error);
    }
    if let Err(error) = crate::backup::lkb::extract_lkb(&bytes, &verify_dir) {
        let _ = std::fs::remove_dir_all(&verify_dir);
        eprintln!("backup: {error}");
        return ExitCode::FAILURE;
    }
    let _ = std::fs::remove_dir_all(&verify_dir);
    println!(
        "backup: {}",
        crate::tr!(
            crate::keys::BACKUP_VERIFIED,
            backup_id = metadata.backup_id,
            label = label
        )
    );
    ExitCode::SUCCESS
}

fn resolve_backup_bytes(
    backup: &Option<String>,
    file: &Option<PathBuf>,
    install_dir: Option<&Path>,
) -> Result<(Vec<u8>, String), plan::InstallError> {
    match (backup, file) {
        (Some(id), None) => {
            if !backup_id_format_ok(id) {
                return Err(plan::InstallError::ParameterUsage(format!(
                    "--backup {id} does not match the backup ID format YYYYMMDD-HHMMSS-<8 lowercase hex>"
                )));
            }
            let root = resolve_root(install_dir)?;
            let path = root.canonical.join("backups").join(format!("{id}.lkb"));
            if !path.is_file() {
                return Err(plan::InstallError::InvalidBackup(format!(
                    "backup {id} not found under {}",
                    root.canonical.join("backups").display()
                )));
            }
            validate_backup_file(&path)?;
            Ok((
                std::fs::read(&path).map_err(plan::InstallError::Io)?,
                format!("backups/{id}.lkb"),
            ))
        }
        (None, Some(path)) => {
            validate_backup_file(path)?;
            Ok((
                std::fs::read(path).map_err(plan::InstallError::Io)?,
                path.display().to_string(),
            ))
        }
        _ => Err(plan::InstallError::ParameterUsage(
            "--backup and --file cannot be combined; one of them is required".into(),
        )),
    }
}

fn resolve_root(install_dir: Option<&Path>) -> Result<InstallRoot, plan::InstallError> {
    let install_root = plan::select_install_root(
        install_dir,
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    crate::deployment::root::normalize_install_root(&install_root)
}

/// 备注来源:`--remark` 优先;未提供时交互模式通过 `/dev/tty` 提示输入,
/// 空回车表示无备注;非交互或无法打开终端时缺省为空。统一校验备注合法性。
fn resolve_remark(remark: &Option<String>) -> Result<String, plan::InstallError> {
    let remark = match remark {
        Some(remark) => remark.clone(),
        None => match crate::interaction::interactive::Tty::open() {
            Ok(mut tty) => tty.input(&crate::tr!(crate::keys::BACKUP_REMARK_PROMPT))?,
            Err(plan::InstallError::NonInteractive(_)) => String::new(),
            Err(error) => return Err(error),
        },
    };
    validate_remark(&remark)?;
    Ok(remark)
}

fn copy_backup_to_output(source: &Path, target: &Path) -> Result<(), plan::InstallError> {
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(plan::InstallError::ParameterUsage(format!(
            "{} already exists",
            target.display()
        )));
    }
    let parent = target.parent().ok_or_else(|| {
        plan::InstallError::ParameterUsage(format!("{} has no parent directory", target.display()))
    })?;
    if !parent.is_dir() {
        return Err(plan::InstallError::ParameterUsage(format!(
            "{} is not a directory",
            parent.display()
        )));
    }
    let tmp = target.with_file_name(format!(
        ".{}.tmp.{}",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(plan::InstallError::Io)?;
    file.write_all(&std::fs::read(source).map_err(plan::InstallError::Io)?)
        .map_err(plan::InstallError::Io)?;
    file.sync_all().map_err(plan::InstallError::Io)?;
    crate::backup::lkb::publish_no_replace(&tmp, target).map_err(|error| {
        if matches!(
            &error,
            plan::InstallError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists
        ) {
            plan::InstallError::ParameterUsage(format!("{} already exists", target.display()))
        } else {
            error
        }
    })
}

fn metadata_bytes_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 32 {
        return None;
    }
    Some(u32::from_le_bytes(bytes[6..10].try_into().ok()?) as usize)
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

#[cfg(feature = "test-support")]
fn resolve_runtime(args: &BackupCreate) -> Result<InstallRuntime, plan::InstallError> {
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(not(feature = "test-support"))]
fn resolve_runtime(_args: &BackupCreate) -> Result<InstallRuntime, plan::InstallError> {
    Ok(InstallRuntime::production())
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

    #[test]
    fn output_refuses_existing_files_and_symlinks() {
        let dir = temp_dir("output");
        let source = dir.join("source.lkb");
        std::fs::write(&source, b"lkb bytes").unwrap();

        let existing = dir.join("existing.lkb");
        std::fs::write(&existing, b"keep").unwrap();
        assert!(matches!(
            copy_backup_to_output(&source, &existing),
            Err(plan::InstallError::ParameterUsage(_))
        ));
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep");

        let link = dir.join("link.lkb");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(copy_backup_to_output(&source, &link).is_err());

        let target = dir.join("out.lkb");
        copy_backup_to_output(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"lkb bytes");
        use std::os::unix::fs::MetadataExt;
        let mode = std::fs::metadata(&target).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remark_resolution_uses_flag_or_empty_default() {
        assert_eq!(
            resolve_remark(&Some("manual note".into())).unwrap(),
            "manual note"
        );
        assert!(matches!(
            resolve_remark(&Some("x".repeat(257))),
            Err(plan::InstallError::ParameterUsage(_))
        ));
        assert!(matches!(
            resolve_remark(&Some("two\nlines".into())),
            Err(plan::InstallError::ParameterUsage(_))
        ));
        crate::interaction::interactive::configure(true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::interaction::interactive::configure(false);
            }
        }
        let _reset = Reset;
        assert_eq!(resolve_remark(&None).unwrap(), "");
        assert_eq!(resolve_remark(&Some("".into())).unwrap(), "");
    }

    #[test]
    fn list_marks_symlinks_and_unsafe_permissions_invalid() {
        let dir = temp_dir("list");
        let source = dir.join("source");
        std::fs::create_dir_all(source.join("static/assets")).unwrap();
        let webserver = source.join("landscape-webserver");
        std::fs::write(&webserver, b"binary").unwrap();
        std::fs::write(source.join("static/index.html"), b"<h1>x</h1>").unwrap();
        std::fs::write(source.join("static.zip"), b"zip").unwrap();
        std::fs::create_dir_all(source.join("geo_tmp")).unwrap();
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = crate::backup::lkb::create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"\n",
            &source.join("static"),
            &source.join("static.zip"),
            &source.join("geo_tmp"),
            "",
            true,
        )
        .unwrap();
        let valid = backups.join(format!("{}.lkb", backup.backup_id));
        std::os::unix::fs::symlink(&valid, backups.join("link.lkb")).unwrap();
        let loose = backups.join("loose.lkb");
        std::fs::copy(&valid, &loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();

        #[cfg(feature = "test-support")]
        let args = BackupList {
            install_dir: Some(dir.clone()),
            test_runtime: None,
        };
        #[cfg(not(feature = "test-support"))]
        let args = BackupList {
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_list(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 构造不含 tar checksum 校验的最小 tar 字节流(用于包装成 `.lkb`)。
    fn raw_tar(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, kind, content) in entries {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}", content.len());
            header[124..124 + 11].copy_from_slice(size.as_bytes());
            header[156] = *kind;
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            let sum: u32 = header.iter().map(|byte| *byte as u32).sum();
            let octal = format!("{sum:06o}");
            header[148..154].copy_from_slice(octal.as_bytes());
            header[154] = 0;
            header[155] = b' ';
            tar.extend_from_slice(&header);
            tar.extend_from_slice(content);
            let pad = (512 - content.len() % 512) % 512;
            tar.extend(std::iter::repeat_n(0, pad));
        }
        tar.extend([0u8; 1024]);
        tar
    }

    /// 把 tar.gz 包装成 checksum 自洽的 `.lkb` 字节(metadata 的 checksum 与归档一致)。
    fn wrap(tar_gz: &[u8]) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(tar_gz);
        let sha256: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let metadata = crate::backup::lkb::BackupMetadata {
            schema_version: 1,
            backup_id: format!("20260801-163000-{}", &sha256[..8]),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            landscape_version: "1.2.3".into(),
            lkit_version: "0.1.0".into(),
            architecture: crate::backup::lkb::BackupArchitecture::X86_64,
            hostname: "test".into(),
            remark: String::new(),
            auto: true,
            scope: crate::backup::lkb::BackupScope::Minimal,
            contents: crate::backup::lkb::BackupContents {
                binary: true,
                static_: true,
                static_archive: true,
                init_config: true,
                geo_cache: true,
            },
            checksum: format!("sha256:{sha256}"),
        };
        let mut bytes = Vec::new();
        let mut header = [0u8; crate::backup::lkb::LKB_HEADER_LEN];
        header[0..4].copy_from_slice(crate::backup::lkb::LKB_MAGIC);
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..10]
            .copy_from_slice(&(serde_json::to_vec(&metadata).unwrap().len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&serde_json::to_vec(&metadata).unwrap());
        bytes.resize(crate::backup::lkb::LKB_METADATA_CAPACITY, 0);
        bytes.extend_from_slice(tar_gz);
        bytes
    }

    /// 写入一个 `root:backups/<id>.lkb` 文件并设为 `0600`。
    #[cfg(feature = "test-support")]
    fn write_backup_file(dir: &std::path::Path, bytes: &[u8]) {
        use std::os::unix::fs::PermissionsExt;
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("20260801-163000-a1b2c3d4.lkb"), bytes).unwrap();
        std::fs::set_permissions(
            backups.join("20260801-163000-a1b2c3d4.lkb"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    #[cfg(feature = "test-support")]
    fn verify_args(dir: &std::path::Path) -> BackupVerify {
        BackupVerify {
            backup: Some("20260801-163000-a1b2c3d4".into()),
            file: None,
            install_dir: Some(dir.to_path_buf()),
            test_runtime: None,
        }
    }

    #[cfg(feature = "test-support")]
    fn leftover_verify_dirs() -> Vec<std::path::PathBuf> {
        std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("lkit-backup-verify-"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn verify_cleans_up_temp_dirs_and_rejects_incomplete_archives() {
        // 完整归档 verify 成功且不留临时解包目录。
        let dir = temp_dir("verify");
        write_backup_file(
            &dir,
            &wrap(&gzip_tar(&raw_tar(&[
                ("landscape-webserver", b'0', b"bin"),
                ("landscape_init.toml", b'0', b"init"),
                ("static.zip", b'0', b"zip"),
                ("static", b'5', b""),
                ("geo_tmp", b'5', b""),
            ]))),
        );
        assert_eq!(run_verify(&verify_args(&dir)), ExitCode::SUCCESS);
        assert!(
            leftover_verify_dirs().is_empty(),
            "verify must clean up its temporary extraction directory"
        );
        // 归档缺少必需条目 landscape_init.toml 时,verify 必须失败且不留临时目录。
        write_backup_file(
            &dir,
            &wrap(&gzip_tar(&raw_tar(&[
                ("landscape-webserver", b'0', b"bin"),
                ("static.zip", b'0', b"zip"),
                ("static", b'5', b""),
                ("geo_tmp", b'5', b""),
            ]))),
        );
        assert_eq!(run_verify(&verify_args(&dir)), ExitCode::FAILURE);
        assert!(
            leftover_verify_dirs().is_empty(),
            "failed verify must not leave temporary extraction directories"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn gzip_tar(mut tar: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        let mut tar_gz = Vec::new();
        let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
        let mut gz = encoder;
        std::io::copy(&mut tar, &mut gz).unwrap();
        gz.finish().unwrap();
        tar_gz
    }

    #[test]
    fn list_marks_content_incomplete_backups_invalid() {
        // 构造 checksum 有效、但缺少 landscape_init.toml 的归档:
        // verify_lkb 通过,内容完整性校验必须拒绝。
        let tar_gz = gzip_tar(&raw_tar(&[
            ("landscape-webserver", b'0', b"bin"),
            ("static.zip", b'0', b"zip"),
            ("static", b'5', b""),
            ("geo_tmp", b'5', b""),
        ]));
        let dir = temp_dir("list-incomplete");
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("20260801-163000-a1b2c3d4.lkb"), wrap(&tar_gz)).unwrap();
        std::fs::set_permissions(
            backups.join("20260801-163000-a1b2c3d4.lkb"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        #[cfg(feature = "test-support")]
        let args = BackupList {
            install_dir: Some(dir.clone()),
            test_runtime: None,
        };
        #[cfg(not(feature = "test-support"))]
        let args = BackupList {
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_list(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&dir);
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
