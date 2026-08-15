use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::deployment::plan::InstallError;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root};
use crate::service::manager::{ManagedService, ServiceManager};
use crate::workflows::install::require_manager;

/// 把 lkit 自身安装为受管服务(`self-service install`)或移除
/// (`self-service remove`)。Phase B 提供垂直切片,验证
/// [`ServiceManager`] trait 的 `LkitDaemon` 定义渲染与注册流程;
/// Phase C 的常驻 daemon 直接复用该安装产物。
#[derive(Debug, Args)]
pub struct SelfService {
    #[command(subcommand)]
    pub action: SelfServiceAction,
}

#[derive(Debug, Subcommand)]
pub enum SelfServiceAction {
    /// 把当前 lkit 可执行文件复制到 <install-root>/service/lkit 并注册为服务
    Install(SelfServiceArgs),
    /// 停止、注销并删除 lkit 服务及其二进制
    Remove(SelfServiceArgs),
}

#[derive(Debug, Args)]
pub struct SelfServiceArgs {
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub fn run(args: &SelfService) -> ExitCode {
    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("self-service: {error}");
            match error {
                // 参数错误与「请求 systemd 但不可用」均属于使用错误。
                InstallError::ParameterUsage(_) | InstallError::Systemd(_) => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn run_inner(args: &SelfService) -> Result<(), InstallError> {
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(InstallError::UnsupportedPlatform(
            "self-service commands require root".into(),
        ));
    }
    let selected = plan::select_install_root(
        sub_args(args).install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let install_root = root::normalize_install_root(&selected)?;
    let _lock = lock::acquire_install_lock(&install_root)?;
    let manager = runtime.service_manager.as_ref();
    match &args.action {
        SelfServiceAction::Install(sub) => install(&install_root, sub, manager),
        SelfServiceAction::Remove(sub) => remove(&install_root, sub, manager),
    }
}

fn install(
    install_root: &root::InstallRoot,
    args: &SelfServiceArgs,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    require_manager(manager)?;
    let service = ManagedService::LkitDaemon;
    let canonical = &install_root.canonical;
    std::fs::create_dir_all(canonical.join("service")).map_err(InstallError::Io)?;
    std::fs::create_dir_all(canonical.join("data")).map_err(InstallError::Io)?;

    let executable = std::env::current_exe().map_err(InstallError::Io)?;
    let binary = canonical.join("service/lkit");
    if std::fs::canonicalize(&executable).ok().as_deref()
        != std::fs::canonicalize(&binary).ok().as_deref()
    {
        std::fs::copy(&executable, &binary).map_err(InstallError::Io)?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .map_err(InstallError::Io)?;
    }

    let content = manager.render_definition(service, canonical)?;
    crate::workflows::install::write_unit_origin(install_root, manager, service, &content)?;
    let origin = canonical
        .join("service")
        .join(manager.service_name(service));
    let result = (|| -> Result<(), InstallError> {
        manager.register(service, &origin)?;
        manager.enable(service)?;
        if manager.is_active(service)? {
            // 旧 daemon 仍在运行:重启使其加载新二进制。
            manager.restart(service)?;
        } else {
            manager.start(service)?;
        }
        let pid = manager.main_pid(service)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "lkit daemon did not produce a main pid after start".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        // 注册/启动失败:尽力恢复现场,避免遗留已启用但未运行的注册服务。
        cleanup_partial_install(manager, service, &binary, &origin);
        return Err(error);
    }
    println!(
        "self-service: {}",
        crate::tr!(crate::keys::SELF_SERVICE_INSTALLED)
    );
    Ok(())
}

/// 注册/启动失败后的尽力清理:停止、注销并删除二进制与定义原件。
fn cleanup_partial_install(
    manager: &dyn ServiceManager,
    service: ManagedService,
    binary: &std::path::Path,
    origin: &std::path::Path,
) {
    let _ = manager.stop(service);
    if manager.is_enabled(service).unwrap_or(false) {
        let _ = manager.disable(service);
    }
    let _ = manager.unregister(service, origin);
    let _ = manager.refresh();
    let _ = remove_file_if_present(binary);
    let _ = remove_file_if_present(origin);
}

fn remove(
    install_root: &root::InstallRoot,
    _args: &SelfServiceArgs,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    let service = ManagedService::LkitDaemon;
    let canonical = &install_root.canonical;
    if manager.is_active(service)? {
        manager.stop_and_wait(
            service,
            &(|| {
                manager
                    .active_state(service)
                    .map(|value| value != "active")
                    .unwrap_or(true)
            }),
        )?;
    }
    if manager.is_enabled(service).unwrap_or(false) {
        let _ = manager.disable(service);
    }
    let origin = canonical
        .join("service")
        .join(manager.service_name(service));
    if let Err(error) = manager.unregister(service, &origin) {
        eprintln!("self-service: {error}");
    }
    remove_file_if_present(&canonical.join("service/lkit"))?;
    remove_file_if_present(&origin)?;
    println!(
        "self-service: {}",
        crate::tr!(crate::keys::SELF_SERVICE_REMOVED)
    );
    Ok(())
}

fn sub_args(args: &SelfService) -> &SelfServiceArgs {
    match &args.action {
        SelfServiceAction::Install(args) => args,
        SelfServiceAction::Remove(args) => args,
    }
}

fn remove_file_if_present(path: &std::path::Path) -> Result<(), InstallError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn resolve_runtime(_args: &SelfService) -> Result<InstallRuntime, InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = sub_args(_args).test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}
