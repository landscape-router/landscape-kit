use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::ExitCode;

use crate::backup::lkb::{BackupMetadata, BackupProgress, validate_remark, verify_lkb};
use crate::deployment::layout;
use crate::deployment::plan;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, state, transaction};
use crate::release::artifacts::WEBSERVER_BINARY;

use super::BackupCreate;
use super::discover_root;
use super::exit_code;

pub(super) async fn run_create(args: &BackupCreate) -> ExitCode {
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
    let root = match discover_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            eprintln!(
                "backup: {}",
                crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION)
            );
            return ExitCode::from(2);
        }
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
    let mut step = None::<crate::interaction::presentation::StepProgress>;
    let mut total = 0u64;
    let result = create_manual_backup(&root, &runtime, &remark, args.output.as_deref(), |p| {
        let progress = step.get_or_insert_with(|| {
            crate::interaction::presentation::StepProgress::new(
                crate::tr!(crate::keys::PRESENTATION_BACKUP_CREATING),
                0,
            )
        });
        match p {
            BackupProgress::Exporting => progress.set_state(
                crate::tr!(crate::keys::PRESENTATION_BACKUP_PROGRESS_EXPORTING),
                0,
                0,
            ),
            BackupProgress::Archiving {
                done,
                total: t,
                current,
            } => {
                total = t;
                progress.set_state(current, done, t);
            }
            BackupProgress::Finalizing => progress.set_state(
                crate::tr!(crate::keys::PRESENTATION_BACKUP_PROGRESS_FINALIZING),
                total,
                total,
            ),
        }
    })
    .await;
    match result {
        Ok(metadata) => {
            if let Some(progress) = step {
                progress.finish();
            }
            let path = if let Some(output) = &args.output {
                output.display().to_string()
            } else {
                layout::territory_backups_dir()
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
            if let Some(progress) = step {
                progress.abandon_failed();
            }
            eprintln!("backup: {error}");
            exit_code(&error)
        }
    }
}

/// CLI 与交互控制台共用的手工备份创建流程：安装锁、中断事务恢复、运行状态
/// 校验、配置导出、归档创建（含文件数进度）与最终自校验。`progress` 在
/// 导出阶段和每个归档文件后收到事件。
pub(crate) async fn create_manual_backup(
    root: &InstallRoot,
    runtime: &InstallRuntime,
    remark: &str,
    output: Option<&Path>,
    mut progress: impl FnMut(BackupProgress),
) -> Result<BackupMetadata, InstallError> {
    let _lock = lock::acquire_install_lock()?;
    let health = runtime.health_options()?;
    let unfinished = transaction::find_unfinished(root)?;
    if let Some(transaction) = unfinished
        && let Err(error) = transaction::recover_interrupted(
            root,
            &transaction,
            runtime.service_manager.as_ref(),
            &health,
        )
        .await
    {
        return Err(error);
    }
    let Some(installed) = state::load_state(root)? else {
        return Err(plan::InstallError::ParameterUsage(crate::tr!(
            crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION
        )));
    };
    crate::workflows::install::check_initialization(root, &installed)?;
    crate::workflows::install::verify_current_backend(root, &installed)?;
    progress(BackupProgress::Exporting);
    let token = crate::backup::export::read_api_token(
        &root.canonical.join("data/landscape_api_token"),
        runtime.managed_uid,
    )?;
    let exported = crate::backup::export::export_config(&runtime.export_base_url, &token).await?;
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
        &layout::territory_backups_dir(),
        &version,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &static_archive,
        &geo_tmp,
        remark,
        false,
        Some(&mut progress),
    )?;
    if let Some(output) = output {
        let final_path = layout::territory_relative(&backup_ref.path);
        copy_backup_to_output(&final_path, output)?;
    }
    let metadata = verify_lkb(
        &std::fs::read(layout::territory_relative(&backup_ref.path))
            .map_err(plan::InstallError::Io)?,
    )?;
    Ok(metadata)
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
        let _interactive_guard = crate::interaction::interactive::test_guard();
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
}
