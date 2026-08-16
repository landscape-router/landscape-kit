use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::workflows::migrate::{MigrateArgs, MigrateOptions, MigrateOutcome};

#[derive(Debug, Args)]
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
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Migrate) -> ExitCode {
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
    let _lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("migrate: {error}");
            return exit_code(&error);
        }
    };
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
    };
    let args = MigrateArgs {
        config_dir: args.from.clone(),
        yes: args.yes,
        console_confirmed: args.console_confirmed,
        repository: super::manage::repository_override(&args.repository),
    };
    match crate::workflows::migrate::migrate_version(
        &normalized,
        runtime.service_manager.as_ref(),
        &args,
        &options,
    )
    .await
    {
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
                    source = args.config_dir.display()
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

    fn migrate_args() -> Migrate {
        Migrate {
            from: "/tmp/legacy-landscape".into(),
            repository: None,
            yes: true,
            install_dir: None,
            console_confirmed: false,
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

    #[tokio::test]
    async fn refuses_migrate_when_an_installation_state_exists() {
        let (_guard, _territory) = territory_with_state("single-instance", valid_state_json());
        let args = migrate_args();
        assert_eq!(run(&args).await, ExitCode::from(2));
    }

    #[tokio::test]
    async fn refuses_migrate_when_the_installation_state_is_corrupted() {
        let (_guard, _territory) = territory_with_state("corrupted-state", b"not json");
        let args = migrate_args();
        assert_eq!(run(&args).await, ExitCode::FAILURE);
    }
}
