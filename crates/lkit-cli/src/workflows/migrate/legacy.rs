use std::path::Path;

use super::super::health::DocsProbe;
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::systemd::{self, Availability, Systemd};
use super::super::transaction::{LegacyUnitBefore, TransactionFile};
use super::rollback::move_file;
use super::{MigrateOptions, pid_alive};

/// 停止旧部署实例并返回其事务前事实。
///
/// systemd 可用时扫描 unit 文件,按 ExecStart 的 `--config-dir` 匹配源目录:
/// - 唯一匹配:stop + disable(原件位于 `/etc/systemd/system` 时移入事务目录,
///   否则 mask,见 [`stop_legacy_unit`]);
/// - 无匹配:实例为前台进程,要求用户确认已停止,lkit 不验证运行态;
/// - 多匹配:阻断,要求用户先手工清理。
///
/// unit 停止后识别出的实例进程仍存活时,同样按前台进程要求用户确认。
pub(crate) fn stop_legacy_instance<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    source: &Path,
    instance: &super::super::process::Process,
    options: &MigrateOptions<'_, P>,
    console_confirmed: bool,
) -> Result<Option<LegacyUnitBefore>, InstallError> {
    if !matches!(systemd.probe(), Availability::Available { .. }) {
        confirm_foreground_stopped(options, console_confirmed)?;
        return Ok(None);
    }
    let units = systemd::find_units_serving_config_dir(systemd, source)?;
    let before = match units.as_slice() {
        [] => {
            confirm_foreground_stopped(options, console_confirmed)?;
            None
        }
        [unit] => Some(stop_legacy_unit(root, transaction, systemd, unit)?),
        _ => {
            return Err(InstallError::ParameterUsage(format!(
                "multiple systemd units serve {}: {}; stop or remove all but one manually and retry",
                source.display(),
                units.join(", ")
            )));
        }
    };
    if pid_alive(instance.pid) {
        confirm_foreground_stopped(options, console_confirmed)?;
    }
    Ok(before)
}

/// 停止并注销单个旧 unit。原件位于 `/etc/systemd/system` 时把 unit 文件原子移入
/// 事务目录(否则 `mask` 会占用该路径,与受管 `landscape-router.service` 注册冲突,
/// 且 systemd 对 `/etc` 下的原件拒绝 mask);其他位置的 unit 走 `stop + disable + mask`。
pub(crate) fn stop_legacy_unit(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    unit: &str,
) -> Result<LegacyUnitBefore, InstallError> {
    let before = systemd::inspect_host_service(systemd, unit)?;
    if !before.installed {
        return Err(InstallError::ParameterUsage(format!(
            "unit {unit} is not installed"
        )));
    }
    let fragment = Path::new(&systemd::fragment_path(systemd, unit)?).to_path_buf();
    let file_moved = fragment.starts_with(&systemd.system_unit_dir);

    systemd::unit_command(systemd, "stop", unit)?;
    if is_enabled_state(&before.enable_state) {
        systemd::unit_command(systemd, "disable", unit)?;
    }
    if file_moved {
        let backup = root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id)
            .join("legacy-unit")
            .join(unit);
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
        }
        move_file(&fragment, &backup)?;
        systemd::daemon_reload(systemd)?;
        Ok(LegacyUnitBefore {
            unit: unit.to_string(),
            installed: true,
            active: before.active,
            enable_state: before.enable_state,
            file_moved: true,
            file_path: Some(fragment.display().to_string()),
            file_backup: Some(format!(
                "transactions/{}/legacy-unit/{unit}",
                transaction.transaction_id
            )),
        })
    } else {
        systemd::unit_command(systemd, "mask", unit)?;
        Ok(LegacyUnitBefore {
            unit: unit.to_string(),
            installed: true,
            active: before.active,
            enable_state: before.enable_state,
            file_moved: false,
            file_path: None,
            file_backup: None,
        })
    }
}

fn is_enabled_state(enable_state: &str) -> bool {
    matches!(
        enable_state,
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias"
    )
}

/// 前台实例停止确认。非交互模式由 `--yes` 覆盖,不再二次确认。
fn confirm_foreground_stopped<P: DocsProbe>(
    options: &MigrateOptions<'_, P>,
    console_confirmed: bool,
) -> Result<(), InstallError> {
    if console_confirmed || super::super::interactive::is_non_interactive() {
        return Ok(());
    }
    let accepted = (options.confirm)(&crate::tr!(crate::keys::MIGRATE_CONFIRM_STOP_FOREGROUND))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused to stop the running instance".into(),
        ));
    }
    Ok(())
}
