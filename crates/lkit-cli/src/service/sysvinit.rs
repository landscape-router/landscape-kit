use std::any::Any;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) use super::manager::{
    Availability, ManagedService, Registration, RegistrationKind, ServiceBefore, ServiceManager,
    ServiceManagerKind, SystemRegistration,
};
use super::plan::InstallError;

pub(crate) const ROUTER_SCRIPT_NAME: &str = "landscape-router.service";
pub(crate) const LKIT_DAEMON_SCRIPT_NAME: &str = "lkit.service";
pub(crate) const INIT_D_DIR: &str = "/etc/init.d";
pub(crate) const RC_D_GLOB: &str = "/etc/rc?.d";

/// sysvinit 后端(简单实现):LSB init 脚本位于 `/etc/init.d/`,注册为指向受管
/// 原件的符号链接;enable/disable 通过 `update-rc.d`,生命周期直接执行脚本的
/// start/stop/restart 目标,运行状态以 `/proc` 命令行为准。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Sysvinit {
    pub update_rc_d: PathBuf,
    pub init_d_dir: PathBuf,
    pub rc_d_glob: PathBuf,
    pub resolv_conf: PathBuf,
}

impl Sysvinit {
    pub(crate) fn host() -> Self {
        Self {
            update_rc_d: PathBuf::from("/usr/sbin/update-rc.d"),
            init_d_dir: PathBuf::from(INIT_D_DIR),
            rc_d_glob: PathBuf::from(RC_D_GLOB),
            resolv_conf: PathBuf::from(super::resolv::RESOLV_CONF),
        }
    }
}

impl ServiceManager for Sysvinit {
    fn kind(&self) -> ServiceManagerKind {
        ServiceManagerKind::Sysvinit
    }

    fn probe(&self) -> Availability {
        probe(self)
    }

    fn service_name(&self, service: ManagedService) -> &str {
        match service {
            ManagedService::LandscapeRouter => ROUTER_SCRIPT_NAME,
            ManagedService::LkitDaemon => LKIT_DAEMON_SCRIPT_NAME,
        }
    }

    fn render_definition(
        &self,
        service: ManagedService,
        canonical_root: &Path,
    ) -> Result<String, InstallError> {
        match service {
            ManagedService::LandscapeRouter => Ok(render_router_script(canonical_root)),
            ManagedService::LkitDaemon => Ok(render_lkit_script(canonical_root)),
        }
    }

    fn validate_definition(
        &self,
        service: ManagedService,
        content: &str,
        canonical_root: &Path,
    ) -> Result<(), InstallError> {
        validate_script(content, service, canonical_root)
    }

    fn query_registration(
        &self,
        service: ManagedService,
    ) -> Result<SystemRegistration, InstallError> {
        query_registration_at(&self.init_d_dir.join(self.service_name(service)))
    }

    fn register(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError> {
        register_at(&self.init_d_dir.join(self.service_name(service)), origin)
    }

    fn unregister(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError> {
        unregister_at(&self.init_d_dir.join(self.service_name(service)), origin)
    }

    fn is_enabled(&self, service: ManagedService) -> Result<bool, InstallError> {
        Ok(rc_d_link_exists(
            &self.rc_d_glob,
            self.service_name(service),
        ))
    }

    fn enable(&self, service: ManagedService) -> Result<(), InstallError> {
        run_update_rc_d(&self.update_rc_d, &["enable", self.service_name(service)])
    }

    fn disable(&self, service: ManagedService) -> Result<(), InstallError> {
        run_update_rc_d(&self.update_rc_d, &["disable", self.service_name(service)])
    }

    fn is_active(&self, service: ManagedService) -> Result<bool, InstallError> {
        Ok(super::manager::pid_of_command(active_pattern(service)).is_some())
    }

    fn active_state(&self, service: ManagedService) -> Result<String, InstallError> {
        Ok(if self.is_active(service)? {
            "active".into()
        } else {
            "inactive".into()
        })
    }

    fn start(&self, service: ManagedService) -> Result<(), InstallError> {
        run_script(&self.init_d_dir.join(self.service_name(service)), "start")
    }

    fn stop(&self, service: ManagedService) -> Result<(), InstallError> {
        run_script(&self.init_d_dir.join(self.service_name(service)), "stop")
    }

    fn restart(&self, service: ManagedService) -> Result<(), InstallError> {
        run_script(&self.init_d_dir.join(self.service_name(service)), "restart")
    }

    fn main_pid(&self, service: ManagedService) -> Result<u32, InstallError> {
        super::manager::pid_of_command(active_pattern(service))
            .map(|pid| pid as u32)
            .ok_or_else(|| {
                InstallError::Systemd(format!(
                    "cannot resolve the main pid of {}",
                    self.service_name(service)
                ))
            })
    }

    fn restore_registration(
        &self,
        service: ManagedService,
        before: &ServiceBefore,
        origin: &Path,
    ) -> Result<(), InstallError> {
        restore_registration_at(
            &self.init_d_dir.join(self.service_name(service)),
            before,
            origin,
        )
    }

    fn resolv_conf(&self) -> &Path {
        &self.resolv_conf
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn probe(sysvinit: &Sysvinit) -> Availability {
    if pid1_is_systemd() {
        return Availability::NotDetected;
    }
    if !sysvinit.init_d_dir.is_dir() {
        return Availability::NotDetected;
    }
    if !is_executable(&sysvinit.update_rc_d) {
        return Availability::Unavailable(format!(
            "{} is missing or not executable",
            sysvinit.update_rc_d.display()
        ));
    }
    Availability::Available {
        version: "sysvinit".into(),
    }
}

fn pid1_is_systemd() -> bool {
    std::fs::read_link("/proc/1/exe")
        .ok()
        .and_then(|path| path.file_name().map(|name| name == "systemd"))
        .unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

fn render_router_script(canonical_root: &Path) -> String {
    format!(
        "#!/bin/sh\n### BEGIN INIT INFO\n# Provides:          landscape-router\n# Required-Start:    $network\n# Required-Stop:     $network\n# Default-Start:     2 3 4 5\n# Default-Stop:      0 1 6\n# Description:       Landscape webserver reverse proxy\n### END INIT INFO\n\ncase \"$1\" in\n  start)\n    start-stop-daemon --start --make-pidfile --pidfile /run/landscape-router.pid --background \\\n      --exec {}/current/landscape-webserver -- --config-dir {}/data --web {}/current/static\n    ;;\n  stop)\n    start-stop-daemon --stop --pidfile /run/landscape-router.pid\n    ;;\n  restart)\n    sh \"$0\" stop\n    sh \"$0\" start\n    ;;\n  *)\n    echo \"Usage: $0 {{start|stop|restart}}\" >&2\n    exit 1\n    ;;\nesac\n",
        shell_quote(&canonical_root.display().to_string()),
        shell_quote(&canonical_root.display().to_string()),
        shell_quote(&canonical_root.display().to_string()),
    )
}

fn render_lkit_script(binary: &Path) -> String {
    format!(
        "#!/bin/sh\n### BEGIN INIT INFO\n# Provides:          lkit\n# Required-Start:    $network\n# Required-Stop:     $network\n# Default-Start:     2 3 4 5\n# Default-Stop:      0 1 6\n# Description:       lkit resident daemon\n### END INIT INFO\n\ncase \"$1\" in\n  start)\n    start-stop-daemon --start --make-pidfile --pidfile /run/lkit.pid --background \\\n      --exec {} -- daemon\n    ;;\n  stop)\n    start-stop-daemon --stop --pidfile /run/lkit.pid\n    ;;\n  restart)\n    sh \"$0\" stop\n    sh \"$0\" start\n    ;;\n  *)\n    echo \"Usage: $0 {{start|stop|restart}}\" >&2\n    exit 1\n    ;;\nesac\n",
        shell_quote(&binary.display().to_string()),
    )
}

fn shell_quote(value: &str) -> String {
    if value.contains([' ', '"', '\'']) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn validate_script(
    content: &str,
    service: ManagedService,
    canonical_root: &Path,
) -> Result<(), InstallError> {
    let expected = match service {
        ManagedService::LandscapeRouter => format!(
            "{}/current/landscape-webserver",
            shell_quote(&canonical_root.display().to_string())
        ),
        ManagedService::LkitDaemon => "/usr/local/bin/lkit".to_string(),
    };
    if !content.contains(&expected) {
        return Err(InstallError::Systemd(format!(
            "service definition no longer points at the managed executable: expected {expected}"
        )));
    }
    if content.contains("PASSWORD") || content.contains("password") {
        return Err(InstallError::Systemd(
            "service definition must not contain credentials".into(),
        ));
    }
    Ok(())
}

fn query_registration_at(path: &Path) -> Result<SystemRegistration, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path).map_err(InstallError::Io)?;
            Ok(SystemRegistration::Symlink { target })
        }
        Ok(metadata) => Ok(SystemRegistration::Conflict {
            file_type: format!("{:?}", metadata.file_type()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SystemRegistration::Missing)
        }
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn register_at(path: &Path, origin: &Path) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
    }
    let temporary = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temporary);
    symlink(origin, &temporary).map_err(InstallError::Io)?;
    std::fs::rename(&temporary, path).map_err(InstallError::Io)
}

fn unregister_at(path: &Path, origin: &Path) -> Result<(), InstallError> {
    match std::fs::read_link(path) {
        Ok(target) if target == origin => std::fs::remove_file(path).map_err(InstallError::Io),
        Ok(_) => Err(InstallError::Systemd(format!(
            "registration {} points at a different origin",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn restore_registration_at(
    path: &Path,
    before: &ServiceBefore,
    origin: &Path,
) -> Result<(), InstallError> {
    match &before.registration.kind {
        RegistrationKind::Missing => unregister_at(path, origin),
        RegistrationKind::Symlink => {
            if std::fs::read_link(path).ok().as_deref() != Some(origin) {
                register_at(path, origin)?;
            }
            Ok(())
        }
    }
}

fn run_script(script: &Path, action: &str) -> Result<(), InstallError> {
    // LSB init 脚本由 sh 执行:定义原件是受管文本文件,不要求可执行位。
    let output = Command::new("sh")
        .arg(script)
        .arg(action)
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", script.display())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallError::Systemd(format!(
        "{} {} failed: {}",
        script.display(),
        action,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn run_update_rc_d(update_rc_d: &Path, args: &[&str]) -> Result<(), InstallError> {
    let output = Command::new(update_rc_d)
        .args(args)
        .output()
        .map_err(|error| {
            InstallError::Systemd(format!("run {}: {error}", update_rc_d.display()))
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallError::Systemd(format!(
        "{} {} failed: {}",
        update_rc_d.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// 在 `/etc/rc?.d` 的每个运行级目录中查找指向 `init.d/<name>` 的 S 链接(enabled)。
fn rc_d_link_exists(rc_d_dir: &Path, name: &str) -> bool {
    let Ok(runlevels) = std::fs::read_dir(rc_d_dir) else {
        return false;
    };
    for runlevel in runlevels.flatten() {
        let directory = runlevel.path();
        if !directory.is_dir() {
            continue;
        }
        let Ok(links) = std::fs::read_dir(&directory) else {
            continue;
        };
        for link in links.flatten() {
            let file_name = link.file_name().to_string_lossy().to_string();
            let Ok(target) = std::fs::read_link(link.path()) else {
                continue;
            };
            if file_name.starts_with('S') && target.to_string_lossy().ends_with(&format!("/{name}"))
            {
                return true;
            }
        }
    }
    false
}

fn active_pattern(service: ManagedService) -> &'static str {
    match service {
        ManagedService::LandscapeRouter => "current/landscape-webserver",
        ManagedService::LkitDaemon => "lkit daemon",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("lkit-sysvinit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn renders_router_script_pointing_at_managed_executable() {
        let root = temp_dir("render");
        let script = render_router_script(&root);
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains(&format!("{}/current/landscape-webserver", root.display())));
        validate_script(&script, ManagedService::LandscapeRouter, &root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renders_lkit_script_and_validates() {
        let root = temp_dir("render-lkit");
        let binary = Path::new("/usr/local/bin/lkit");
        let script = render_lkit_script(binary);
        assert!(script.contains("/usr/local/bin/lkit"));
        assert!(script.contains("daemon"));
        assert!(!script.contains(&root.display().to_string()));
        validate_script(&script, ManagedService::LkitDaemon, &root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_rejects_tampered_definition() {
        let root = temp_dir("validate");
        let script = render_router_script(&root).replace(
            &format!("{}/current/landscape-webserver", root.display()),
            "/tmp/other-webserver",
        );
        assert!(validate_script(&script, ManagedService::LandscapeRouter, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rc_d_link_detection() {
        let dir = temp_dir("rc-d");
        let rc3 = dir.join("rc3.d");
        std::fs::create_dir_all(&rc3).unwrap();
        std::fs::write(rc3.join("README"), "do not touch\n").unwrap();
        assert!(!rc_d_link_exists(&dir, "landscape-router"));
        symlink(
            Path::new("/etc/init.d/landscape-router"),
            rc3.join("S20landscape-router"),
        )
        .unwrap();
        assert!(rc_d_link_exists(&dir, "landscape-router"));
        assert!(!rc_d_link_exists(&dir, "other"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registration_link_lifecycle() {
        let dir = temp_dir("registration");
        let origin = dir.join("service/lkit");
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::write(&origin, "#!/bin/sh\n").unwrap();
        let path = dir.join("init.d/lkit");
        register_at(&path, &origin).unwrap();
        assert!(matches!(
            query_registration_at(&path).unwrap(),
            SystemRegistration::Symlink { .. }
        ));
        unregister_at(&path, &origin).unwrap();
        assert_eq!(
            query_registration_at(&path).unwrap(),
            SystemRegistration::Missing
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
