use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use crate::daemon_worker::{self, DelegateError, DelegationBlock};
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::interaction::presentation::InterruptGuard;
use crate::workflows::migrate::{MigrateArgs, MigrateOptions, MigrateOutcome};

#[derive(Clone, Debug, Args)]
pub struct Migrate {
    /// 旧手工部署的 Landscape 配置目录(如 /root/.landscape-router)
    #[arg(long, value_name = "CONFIG_DIR")]
    pub from: PathBuf,
    /// 仅用于 static.zip 缺失时从发布仓库下载该版本
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    /// 非交互模式确认迁移
    #[arg(long)]
    pub yes: bool,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    /// 控制台已确认迁移计划(内部参数,交互模式也跳过 tty 确认)
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    /// 前台已准备好的迁移事务 id(内部参数:daemon worker 据此跳过前置检查,
    /// 只执行切换阶段)
    #[arg(long = "resume", value_name = "TRANSACTION_ID", hide = true)]
    pub resume_transaction: Option<String>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Migrate, interrupt: &InterruptGuard, from_console: bool) -> ExitCode {
    // 单实例守卫:lkit 地盘已有有效安装状态时拒绝迁移(无论 --install-dir),
    // 必须先卸载;损坏状态直接报错。
    match state::read_state() {
        Ok(Some(_)) => {
            eprintln!(
                "migrate: {}",
                crate::tr!(crate::keys::MIGRATE_SINGLE_INSTANCE_REFUSED)
            );
            return ExitCode::from(2);
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    }
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "migrate: {}",
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT)
        );
        return ExitCode::FAILURE;
    }
    let install_root = match plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    ) {
        Ok(install_root) => install_root,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
    let normalized = match root::normalize_install_root(&install_root) {
        Ok(normalized) => normalized,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
    // worker 恢复路径是前台迁移的内部延续:发起进程持有安装锁直到委托完成,
    // worker 不能再取锁(flock 非阻塞),否则立即 LockBusy。
    let _lock = if args.resume_transaction.is_none() {
        match lock::acquire_install_lock() {
            Ok(lock) => Some(lock),
            Err(error) => {
                eprintln!("migrate: {error}");
                return exit_code(&error);
            }
        }
    } else {
        None
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
    // worker 恢复路径的事务就是我们要继续的 prepared 迁移,不能走中断恢复;
    // 前台路径恢复此前中断的事务(含遗落的 prepared 事务)。
    if args.resume_transaction.is_none() {
        let unfinished = match transaction::find_unfinished(&normalized) {
            Ok(transaction) => transaction,
            Err(error) => {
                eprintln!("migrate: {error}");
                return exit_code(&error);
            }
        };
        if let Some(transaction) = unfinished
            && let Err(error) = transaction::recover_interrupted(
                &normalized,
                &transaction,
                runtime.service_manager.as_ref(),
                &health,
            )
            .await
        {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    }
    match state::load_state(&normalized) {
        Ok(Some(_)) => {
            eprintln!(
                "migrate: {}",
                crate::tr!(crate::keys::MIGRATE_REQUIRES_FRESH_INSTALL_ROOT)
            );
            return ExitCode::from(2);
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    }
    if let Err(error) = reject_leftover_content(&normalized) {
        eprintln!("migrate: {error}");
        return exit_code(&error);
    }
    let options = MigrateOptions {
        export_base_url: runtime.export_base_url.clone(),
        managed_uid: runtime.managed_uid,
        confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
        health: &health,
        probe_ports: &health.ports,
        interrupted: &|| interrupt.requested(),
    };
    let workflow_args = MigrateArgs {
        config_dir: args.from.clone(),
        yes: args.yes,
        console_confirmed: args.console_confirmed,
        repository: super::manage::repository_override(&args.repository),
        resume_transaction: args.resume_transaction.clone(),
    };
    if workflow_args.resume_transaction.is_some() {
        // daemon worker 恢复路径:前置检查已由前台完成,只执行切换阶段。
        // SIGTERM 是 daemon 的取消通道,处理器把它转成回滚请求(见
        // workflows::migrate 的检查点),而不是默认终止进程。
        crate::workflows::migrate::install_sigterm_handler();
        return finish_outcome(
            crate::workflows::migrate::resume_migrate_switch(
                &normalized,
                runtime.service_manager.as_ref(),
                &workflow_args,
                &options,
            )
            .await,
            &workflow_args.config_dir,
        );
    }
    // 前台路径:root + daemon 时前置检查在当前进程执行,切换阶段委托 worker;
    // 非 root / 测试 runtime 整条流程内联执行。
    if daemon_worker::migrate_delegates(&super::Commands::Migrate(args.clone())) {
        if let Some(block) = daemon_worker::delegation_block() {
            eprintln!("migrate: {}", block.message());
            return ExitCode::from(2);
        }
        let prepared = match crate::workflows::migrate::prepare_migration(
            &normalized,
            runtime.service_manager.as_ref(),
            &workflow_args,
            &options,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                eprintln!("migrate: {error}");
                return exit_code(&error);
            }
        };
        eprintln!("migrate: {}", crate::tr!(crate::keys::MIGRATE_HANDING_OVER));
        let worker_args = worker_args(args, &prepared.transaction_id);
        return match daemon_worker::delegate(interrupt, worker_args, None, None, from_console).await
        {
            Ok(code) => code,
            Err(error) => {
                let exit = delegate_error_exit(&error);
                eprintln!(
                    "migrate: {}",
                    crate::tr!(
                        crate::keys::MIGRATE_PREPARED_LEFT_BEHIND,
                        transaction_id = prepared.transaction_id
                    )
                );
                exit
            }
        };
    }
    finish_outcome(
        crate::workflows::migrate::migrate_version(
            &normalized,
            runtime.service_manager.as_ref(),
            &workflow_args,
            &options,
        )
        .await,
        &workflow_args.config_dir,
    )
}

/// 构造 daemon worker 的委托参数:前置检查已在发起进程完成,worker 只认领
/// `--resume <事务 id>` 执行切换。`--console-confirmed` 无条件携带(委托只
/// 发生在计划确认之后,worker 进程无法读取 TUI 输入),`--non-interactive`/
/// `--yes` 与 `--test-runtime`(execution=daemon 的测试 runtime,worker 需要
/// 它解析 fake systemd/运行时)按发起会话透传,保证 worker 的确认语义一致。
fn worker_args(args: &Migrate, transaction_id: &str) -> Vec<String> {
    let mut worker_args = vec![
        "migrate".to_string(),
        "--from".to_string(),
        args.from.display().to_string(),
    ];
    if let Some(dir) = &args.install_dir {
        worker_args.push("--install-dir".to_string());
        worker_args.push(dir.display().to_string());
    }
    if args.yes {
        worker_args.push("--yes".to_string());
    }
    worker_args.push("--console-confirmed".to_string());
    if crate::interaction::interactive::is_non_interactive() {
        worker_args.push("--non-interactive".to_string());
    }
    #[cfg(feature = "test-support")]
    if let Some(path) = &args.test_runtime {
        worker_args.push("--test-runtime".to_string());
        worker_args.push(path.display().to_string());
    }
    worker_args.push("--resume".to_string());
    worker_args.push(transaction_id.to_string());
    worker_args
}

fn finish_outcome(outcome: Result<MigrateOutcome, plan::InstallError>, source: &Path) -> ExitCode {
    match outcome {
        Ok(MigrateOutcome::Committed { version, backup_id }) => {
            println!(
                "migrate: {}",
                crate::tr!(
                    crate::keys::MIGRATE_COMMITTED,
                    version = version,
                    backup_id = backup_id
                )
            );
            println!(
                "migrate: {}",
                crate::tr!(
                    crate::keys::MIGRATE_LEGACY_DEPLOYMENT_LEFT,
                    source = source.display()
                )
            );
            ExitCode::SUCCESS
        }
        Ok(MigrateOutcome::RolledBack { version }) => {
            eprintln!(
                "migrate: {}",
                crate::tr!(crate::keys::MIGRATE_FAILED_ROLLED_BACK, version = version)
            );
            ExitCode::from(5)
        }
        Ok(MigrateOutcome::Cancelled { version }) => {
            eprintln!(
                "migrate: {}",
                crate::tr!(
                    crate::keys::MIGRATE_CANCELLED_ROLLED_BACK,
                    version = version
                )
            );
            ExitCode::from(130)
        }
        Ok(MigrateOutcome::RollbackFailed { version, reason }) => {
            eprintln!(
                "migrate: {}",
                crate::tr!(
                    crate::keys::MIGRATE_FAILED_ROLLBACK_FAILED,
                    version = version,
                    reason = reason
                )
            );
            ExitCode::from(6)
        }
        Err(error) => {
            eprintln!("migrate: {error}");
            exit_code(&error)
        }
    }
}

fn delegate_error_exit(error: &DelegateError) -> ExitCode {
    match error {
        DelegateError::Usage(message) => {
            eprintln!("migrate: {message}");
            ExitCode::from(2)
        }
        DelegateError::Infrastructure(message) => {
            eprintln!("migrate: {message}");
            ExitCode::FAILURE
        }
    }
}

impl DelegationBlock {
    /// 委托前置条件阻断时给用户的提示(与 daemon_worker::delegate 的报错一致)。
    fn message(self) -> &'static str {
        match self {
            DelegationBlock::DaemonNotRunning => {
                "the lkit daemon is not running; deploy it with `lkit self install`"
            }
            DelegationBlock::WorkerSpawnUnavailable => {
                "the lkit daemon cannot spawn worker commands: its executable was deleted or replaced; restore the executable and restart the daemon"
            }
        }
    }
}

/// 迁移目标根必须没有遗留受管内容:无 state 但存在 data/releases/service/current
/// 属于上次未清理的现场,阻断并要求手工处理。
fn reject_leftover_content(normalized: &root::InstallRoot) -> Result<(), plan::InstallError> {
    for dir in ["data", "releases", "service"] {
        if normalized.canonical.join(dir).exists() {
            return Err(plan::InstallError::ParameterUsage(format!(
                "the install root contains leftover managed content {}; clean it manually or choose a different --install-dir",
                normalized.canonical.join(dir).display()
            )));
        }
    }
    if std::fs::symlink_metadata(normalized.canonical.join("current")).is_ok() {
        return Err(plan::InstallError::ParameterUsage(
            "the install root contains a leftover current link; clean it manually or choose a different --install-dir"
                .into(),
        ));
    }
    Ok(())
}

fn exit_code(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) | plan::InstallError::UnsupportedPlatform(_) => {
            ExitCode::from(2)
        }
        _ => ExitCode::FAILURE,
    }
}

#[cfg(feature = "test-support")]
fn resolve_runtime(args: &Migrate) -> Result<InstallRuntime, plan::InstallError> {
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(not(feature = "test-support"))]
fn resolve_runtime(_args: &Migrate) -> Result<InstallRuntime, plan::InstallError> {
    Ok(InstallRuntime::production())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::layout;
    use crate::interaction::presentation::InterruptGuard;

    fn migrate_args() -> Migrate {
        Migrate {
            from: "/tmp/legacy-landscape".into(),
            repository: None,
            yes: true,
            install_dir: None,
            console_confirmed: false,
            resume_transaction: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        }
    }

    /// 建立隔离 lkit 地盘,写入指定状态内容,返回 (守卫, 地盘)。
    fn territory_with_state(name: &str, bytes: &[u8]) -> (layout::TerritoryOverride, PathBuf) {
        let territory = std::env::temp_dir().join(format!(
            "lkit-migrate-command-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("state")).unwrap();
        let guard = layout::test_territory(&territory);
        std::fs::write(layout::territory_state_path(), bytes).unwrap();
        (guard, territory)
    }

    fn valid_state_json() -> &'static [u8] {
        br#"{"schema_version":1,"layout_version":2,"install_root":"/opt/landscape","canonical_install_root":"/opt/landscape","active_version":"0.19.2","assets":{"webserver":{"architecture":"x86_64","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":10},"static_archive":{"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":20}},"initialization":{"status":"complete","lock_present":true,"initialized_at":"2026-08-01T16:30:00Z"},"service":{"manager":"systemd","registered":true,"enabled":true,"verified":true,"definition_path":"service/landscape-router.service","definition_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"last_transaction_id":null,"committed_at":"2026-08-01T16:30:00Z"}"#
    }

    /// migrate 命令测试与 workflows::migrate 的测试共用交互互斥,
    /// 避免并行安装 SIGINT 处理器冲突。
    fn interactive_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::interaction::interactive::test_guard()
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refuses_migrate_when_an_installation_state_exists() {
        let _guard = interactive_guard();
        let interrupt = InterruptGuard::install(false).unwrap();
        let (_guard, _territory) = territory_with_state("single-instance", valid_state_json());
        let args = migrate_args();
        assert_eq!(run(&args, &interrupt, false).await, ExitCode::from(2));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn refuses_migrate_when_the_installation_state_is_corrupted() {
        let _guard = interactive_guard();
        let interrupt = InterruptGuard::install(false).unwrap();
        let (_guard, _territory) = territory_with_state("corrupted-state", b"not json");
        let args = migrate_args();
        assert_eq!(run(&args, &interrupt, false).await, ExitCode::FAILURE);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn resume_without_a_prepared_transaction_fails() {
        let _guard = interactive_guard();
        let interrupt = InterruptGuard::install(false).unwrap();
        let territory =
            std::env::temp_dir().join(format!("lkit-migrate-resume-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&territory);
        let _guard = layout::test_territory(&territory);
        let mut args = migrate_args();
        args.install_dir = Some(territory.join("install"));
        args.resume_transaction = Some("missing-transaction".into());
        let code = run(&args, &interrupt, false).await;
        assert_eq!(code, ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&territory);
    }

    #[tokio::test]
    async fn worker_args_carry_the_prepared_transaction() {
        // console_confirmed 无条件携带:委托只发生在计划确认之后。
        let args = Migrate {
            from: "/srv/landscape".into(),
            repository: Some(None),
            yes: true,
            install_dir: Some("/opt/landscape".into()),
            console_confirmed: false,
            resume_transaction: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        };
        let worker_args = worker_args(&args, "tx-123");
        assert_eq!(
            worker_args,
            [
                "migrate",
                "--from",
                "/srv/landscape",
                "--install-dir",
                "/opt/landscape",
                "--yes",
                "--console-confirmed",
                "--resume",
                "tx-123",
            ]
        );
    }

    /// 切换期间取消 → 回滚恢复旧实例 → 退出码 130(与 ^C 约定一致)。
    #[test]
    fn cancelled_outcome_exits_130() {
        let code = finish_outcome(
            Ok(MigrateOutcome::Cancelled {
                version: semver::Version::new(0, 23, 0),
            }),
            Path::new("/tmp/legacy-landscape"),
        );
        assert_eq!(code, ExitCode::from(130));
    }

    /// worker 委托参数里的 `--resume <事务 id>` 必须能被 clap 解析到
    /// `resume_transaction` 字段,否则 daemon worker 会拒绝内部参数。
    #[test]
    fn parses_the_internal_resume_flag() {
        use clap::{Command, FromArgMatches};
        let command = <Migrate as Args>::augment_args(Command::new("migrate"));
        let matches = command
            .try_get_matches_from([
                "migrate",
                "--from",
                "/root/.landscape-router",
                "--resume",
                "tx-456",
            ])
            .unwrap();
        let migrate = Migrate::from_arg_matches(&matches).unwrap();
        assert_eq!(migrate.resume_transaction.as_deref(), Some("tx-456"));
    }

    /// 委托参数透传 `--test-runtime`(execution=daemon 时 worker 用它解析
    /// fake systemd/运行时,与 install/switch 等委托命令的原始参数一致)。
    #[test]
    fn worker_args_carry_the_test_runtime() {
        #[cfg(feature = "test-support")]
        {
            let args = Migrate {
                from: "/srv/landscape".into(),
                repository: None,
                yes: false,
                install_dir: None,
                console_confirmed: false,
                resume_transaction: None,
                test_runtime: Some("/tmp/runtime-daemon.json".into()),
            };
            let worker_args = worker_args(&args, "tx-789");
            assert!(
                worker_args
                    .windows(2)
                    .any(|pair| pair == ["--test-runtime", "/tmp/runtime-daemon.json"]),
                "worker args must forward the test runtime: {worker_args:?}"
            );
        }
    }
}
