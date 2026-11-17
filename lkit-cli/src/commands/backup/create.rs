use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
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

/// CLI 与交互控制台共用的手工备份创建流程：安装锁、未完成事务阻断、运行状态
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
    if let Some(transaction) = transaction::find_unfinished(root)? {
        return Err(InstallError::BlockedByTransaction(format!(
            "{} is {}; backup does not recover transactions. Wait for the lkit daemon to finish recovery or network confirmation; if the daemon is unavailable, restore its service first",
            transaction.transaction_id,
            transaction.phase.key()
        )));
    }
    // Reserve a private sibling before exporting or creating a local backup.
    // This rejects predictable destination errors without leaving a hidden backup.
    let output = output.map(BackupOutput::prepare).transpose()?;
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
        crate::deployment::state::StateArchitecture::Riscv64 => "riscv64",
        crate::deployment::state::StateArchitecture::Loongarch64 => "loongarch64",
    };
    let webserver = root
        .canonical
        .join("releases")
        .join(&installed.active_version)
        .join(WEBSERVER_BINARY);
    let static_dir = root.canonical.join("current/static");
    let geo_tmp = root.canonical.join("data/geo_tmp");
    let backup_ref = crate::backup::lkb::create_backup(
        &layout::territory_backups_dir(),
        &version,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &geo_tmp,
        remark,
        false,
        Some(&mut progress),
    )?;
    let final_path = layout::territory_relative(&backup_ref.path);
    let metadata = verify_lkb(&std::fs::read(&final_path).map_err(plan::InstallError::Io)?)?;
    if let Some(output) = output {
        output.publish(&final_path).map_err(|error| {
            InstallError::Io(std::io::Error::other(format!(
                "backup output failed: {error}; verified local backup {} is retained at {}",
                metadata.backup_id,
                final_path.display()
            )))
        })?;
    }
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

struct BackupOutput {
    target: PathBuf,
    temporary: PathBuf,
    file: File,
}

impl BackupOutput {
    fn prepare(target: &Path) -> Result<Self, InstallError> {
        Self::prepare_in(target, &std::env::current_dir().map_err(InstallError::Io)?)
    }

    fn prepare_in(target: &Path, working_directory: &Path) -> Result<Self, InstallError> {
        let target = working_directory.join(target);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(InstallError::ParameterUsage(format!(
                    "{} already exists",
                    target.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
        let parent = target.parent().ok_or_else(|| {
            InstallError::ParameterUsage(format!("{} has no parent directory", target.display()))
        })?;
        if !parent.is_dir() {
            return Err(InstallError::ParameterUsage(format!(
                "{} is not a directory",
                parent.display()
            )));
        }
        let temporary = parent.join(format!(".lkit-backup-{}.tmp", uuid::Uuid::now_v7()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(InstallError::Io)?;
        Ok(Self {
            target,
            temporary,
            file,
        })
    }

    fn publish(mut self, source: &Path) -> Result<(), InstallError> {
        let mut source = File::open(source).map_err(InstallError::Io)?;
        std::io::copy(&mut source, &mut self.file).map_err(InstallError::Io)?;
        self.file.sync_all().map_err(InstallError::Io)?;
        crate::backup::lkb::publish_no_replace(&self.temporary, &self.target)?;
        File::open(self.target.parent().unwrap())
            .and_then(|directory| directory.sync_all())
            .map_err(InstallError::Io)
    }
}

impl Drop for BackupOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
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
            BackupOutput::prepare(&existing),
            Err(plan::InstallError::ParameterUsage(_))
        ));
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep");

        let link = dir.join("link.lkb");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(BackupOutput::prepare(&link).is_err());

        let target = dir.join("out.lkb");
        BackupOutput::prepare(&target)
            .unwrap()
            .publish(&source)
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"lkb bytes");
        use std::os::unix::fs::MetadataExt;
        let mode = std::fs::metadata(&target).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_supports_bare_relative_names_and_cleans_temporary_files_on_failure() {
        let dir = temp_dir("relative-output");
        let source = dir.join("source.lkb");
        std::fs::write(&source, b"backup").unwrap();
        BackupOutput::prepare_in(Path::new("backup.lkb"), &dir)
            .unwrap()
            .publish(&source)
            .unwrap();
        assert_eq!(std::fs::read(dir.join("backup.lkb")).unwrap(), b"backup");

        let raced = BackupOutput::prepare_in(Path::new("raced.lkb"), &dir).unwrap();
        let temporary = raced.temporary.clone();
        std::fs::write(dir.join("raced.lkb"), b"keep").unwrap();
        assert!(raced.publish(&source).is_err());
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(dir.join("raced.lkb")).unwrap(), b"keep");

        let missing = BackupOutput::prepare_in(Path::new("missing.lkb"), &dir).unwrap();
        let temporary = missing.temporary.clone();
        assert!(missing.publish(&dir.join("absent-source")).is_err());
        assert!(!temporary.exists());
        assert!(!dir.join("missing.lkb").exists());
        std::fs::remove_dir_all(dir).unwrap();
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

    #[cfg(feature = "test-support")]
    mod workflow {
        use super::*;
        use crate::commands::backup::tests::{export_ok_server, fake_install, runtime_file, setup};
        use crate::deployment::transaction::{Phase, TransactionFile};
        use crate::service::manager::{
            Availability, ManagedService, ServiceBefore, ServiceManager, ServiceManagerKind,
            SystemRegistration,
        };
        use std::os::unix::fs::PermissionsExt;

        struct NoServiceCalls;
        macro_rules! forbid_service_call {
            ($name:ident($($argument:ident: $kind:ty),*) -> $result:ty) => {
                fn $name(&self, $(_: $kind),*) -> $result {
                    panic!("backup must not call service manager {}", stringify!($name))
                }
            };
        }
        impl ServiceManager for NoServiceCalls {
            forbid_service_call!(kind() -> ServiceManagerKind);
            forbid_service_call!(probe() -> Availability);
            forbid_service_call!(service_name(service: ManagedService) -> &str);
            forbid_service_call!(render_definition(service: ManagedService, root: &Path) -> Result<String, InstallError>);
            forbid_service_call!(validate_definition(service: ManagedService, content: &str, root: &Path) -> Result<(), InstallError>);
            forbid_service_call!(query_registration(service: ManagedService) -> Result<SystemRegistration, InstallError>);
            forbid_service_call!(register(service: ManagedService, origin: &Path) -> Result<(), InstallError>);
            forbid_service_call!(unregister(service: ManagedService, origin: &Path) -> Result<(), InstallError>);
            forbid_service_call!(is_enabled(service: ManagedService) -> Result<bool, InstallError>);
            forbid_service_call!(enable(service: ManagedService) -> Result<(), InstallError>);
            forbid_service_call!(disable(service: ManagedService) -> Result<(), InstallError>);
            forbid_service_call!(is_active(service: ManagedService) -> Result<bool, InstallError>);
            forbid_service_call!(active_state(service: ManagedService) -> Result<String, InstallError>);
            forbid_service_call!(start(service: ManagedService) -> Result<(), InstallError>);
            forbid_service_call!(stop(service: ManagedService) -> Result<(), InstallError>);
            forbid_service_call!(restart(service: ManagedService) -> Result<(), InstallError>);
            forbid_service_call!(main_pid(service: ManagedService) -> Result<u32, InstallError>);
            forbid_service_call!(restore_registration(service: ManagedService, before: &ServiceBefore, origin: &Path) -> Result<(), InstallError>);
            forbid_service_call!(resolv_conf() -> &Path);
            forbid_service_call!(as_any() -> &dyn std::any::Any);
        }

        fn runtime(temp: &Path, export_base: &str) -> InstallRuntime {
            let mut runtime =
                InstallRuntime::from_test_file(&runtime_file(temp, export_base)).unwrap();
            runtime.service_manager = Box::new(NoServiceCalls);
            runtime
        }

        fn install(root: &Path) -> InstallRoot {
            let installed = fake_install(root);
            let root = InstallRoot {
                install_root: root.into(),
                canonical: root.into(),
            };
            state::write_state(&root, &installed).unwrap();
            let token = root.canonical.join("data/landscape_api_token");
            std::fs::write(&token, b"tok\n").unwrap();
            std::fs::set_permissions(token, std::fs::Permissions::from_mode(0o400)).unwrap();
            root
        }

        #[tokio::test]
        async fn unfinished_transactions_block_without_recovery_or_service_calls() {
            let (_guard, territory, directory) = setup("create-unfinished");
            let root = install(&directory);
            let runtime = runtime(territory.parent().unwrap(), "http://127.0.0.1:1");
            let installed_before = std::fs::read(layout::territory_state_path()).unwrap();
            let current_before = std::fs::read_link(directory.join("current")).unwrap();
            let version = semver::Version::new(1, 2, 3);
            let output = territory.parent().unwrap().join("output.lkb");
            let transactions = [
                (
                    TransactionFile::new_restore(&root, &version, &version).unwrap(),
                    Phase::Preparing,
                ),
                (
                    TransactionFile::new_restore(&root, &version, &version).unwrap(),
                    Phase::Activating,
                ),
                (
                    TransactionFile::new_restore(&root, &version, &version).unwrap(),
                    Phase::RollingBack,
                ),
                (
                    TransactionFile::new_uninstall(&root, &version).unwrap(),
                    Phase::Activating,
                ),
                (
                    TransactionFile::new_reinit(&root, &version).unwrap(),
                    Phase::AwaitingNetworkConfirmation,
                ),
            ];
            for (mut tx, phase) in transactions {
                tx.phase = phase;
                tx.no_backup = true;
                tx.schema_version = match tx.operation {
                    transaction::Operation::Restore => {
                        tx.restore_backup = Some(transaction::BackupRef {
                            backup_id: "20261117-120000-12345678".into(),
                            path: "backups/20261117-120000-12345678.lkb".into(),
                            sha256: "a".repeat(64),
                        });
                        9
                    }
                    transaction::Operation::Uninstall => 10,
                    transaction::Operation::Reinit => 11,
                    _ => unreachable!(),
                };
                transaction::persist(&root, &tx).unwrap();
                let tx_path = layout::territory_transactions_dir()
                    .join(format!("{}.json", tx.transaction_id));
                let before = std::fs::read(&tx_path).unwrap();
                let evidence = layout::territory_transactions_dir().join(&tx.transaction_id);
                std::fs::create_dir_all(&evidence).unwrap();
                std::fs::write(evidence.join("keep"), b"recovery evidence").unwrap();
                let error = create_manual_backup(&root, &runtime, "", Some(&output), |_| {
                    panic!("blocked backup must not export or archive");
                })
                .await
                .unwrap_err();
                assert!(
                    matches!(error, InstallError::BlockedByTransaction(_)),
                    "{error}"
                );
                assert!(error.to_string().contains("lkit daemon"));
                assert_eq!(std::fs::read(&tx_path).unwrap(), before);
                assert_eq!(
                    std::fs::read(evidence.join("keep")).unwrap(),
                    b"recovery evidence"
                );
                assert_eq!(
                    std::fs::read(layout::territory_state_path()).unwrap(),
                    installed_before
                );
                assert_eq!(
                    std::fs::read_link(directory.join("current")).unwrap(),
                    current_before
                );
                assert!(!territory.join("backups").exists());
                assert!(!output.exists());
                assert!(!territory.join("run/daemon.pid").exists());
                std::fs::remove_file(tx_path).unwrap();
            }
            std::fs::remove_dir_all(territory.parent().unwrap()).unwrap();
        }

        #[tokio::test]
        async fn bad_output_is_rejected_before_export_or_local_backup() {
            let (_guard, territory, directory) = setup("create-bad-output");
            let root = install(&directory);
            let runtime = runtime(territory.parent().unwrap(), "http://127.0.0.1:1");
            let existing = territory.parent().unwrap().join("existing.lkb");
            std::fs::write(&existing, b"keep").unwrap();
            let link = territory.parent().unwrap().join("link.lkb");
            std::os::unix::fs::symlink(&existing, &link).unwrap();
            for output in [
                existing.clone(),
                link,
                territory.parent().unwrap().join("missing/out.lkb"),
            ] {
                let error = create_manual_backup(&root, &runtime, "", Some(&output), |_| {
                    panic!("invalid output must be rejected before export");
                })
                .await
                .unwrap_err();
                assert!(matches!(error, InstallError::ParameterUsage(_)), "{error}");
                assert!(!territory.join("backups").exists());
                assert_eq!(std::fs::read(&existing).unwrap(), b"keep");
            }
            std::fs::remove_dir_all(territory.parent().unwrap()).unwrap();
        }

        #[tokio::test]
        async fn late_output_collision_retains_and_reports_verified_local_backup() {
            let (_guard, territory, directory) = setup("create-output-race");
            let root = install(&directory);
            let server = export_ok_server("1.2.3");
            let runtime = runtime(territory.parent().unwrap(), &server.base);
            let output = territory.parent().unwrap().join("output.lkb");
            let error = create_manual_backup(&root, &runtime, "", Some(&output), |event| {
                if matches!(event, BackupProgress::Exporting) {
                    std::fs::write(&output, b"other writer").unwrap();
                }
            })
            .await
            .unwrap_err();
            assert!(matches!(error, InstallError::Io(_)), "{error}");
            assert_eq!(std::fs::read(&output).unwrap(), b"other writer");
            let backup = std::fs::read_dir(territory.join("backups"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|extension| extension == "lkb"))
                .unwrap();
            let metadata = verify_lkb(&std::fs::read(&backup).unwrap()).unwrap();
            let message = error.to_string();
            assert!(message.contains(&metadata.backup_id), "{message}");
            assert!(message.contains(&backup.display().to_string()), "{message}");
            assert!(message.contains("retained"), "{message}");
            assert!(
                std::fs::read_dir(territory.parent().unwrap())
                    .unwrap()
                    .all(|entry| {
                        !entry
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".lkit-backup-")
                    })
            );
            std::fs::remove_dir_all(territory.parent().unwrap()).unwrap();
        }
    }
}
