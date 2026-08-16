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

/// OpenRC 后端:通过 rc-service/rc-update 管理生命周期,服务定义是
/// `/etc/init.d/` 下指向受管原件的符号链接(简单实现,未覆盖 runlevels 之外的
/// OpenRC 特性;复杂能力由真实操作驱动契约演进)。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Openrc {
    pub rc_service: PathBuf,
    pub rc_update: PathBuf,
    pub init_d_dir: PathBuf,
    pub resolv_conf: PathBuf,
}

impl Openrc {
    pub(crate) fn host() -> Self {
        Self {
            rc_service: PathBuf::from("/sbin/rc-service"),
            rc_update: PathBuf::from("/sbin/rc-update"),
            init_d_dir: PathBuf::from(INIT_D_DIR),
            resolv_conf: PathBuf::from(super::resolv::RESOLV_CONF),
        }
    }
}

impl ServiceManager for Openrc {
    fn kind(&self) -> ServiceManagerKind {
        ServiceManagerKind::Openrc
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
        rc_update_show(&self.rc_update, self.service_name(service))
    }

    fn enable(&self, service: ManagedService) -> Result<(), InstallError> {
        run_rc_update(
            &self.rc_update,
            &["add", self.service_name(service), "default"],
        )
    }

    fn disable(&self, service: ManagedService) -> Result<(), InstallError> {
        run_rc_update(
            &self.rc_update,
            &["del", self.service_name(service), "default"],
        )
    }

    fn is_active(&self, service: ManagedService) -> Result<bool, InstallError> {
        rc_service_status(&self.rc_service, self.service_name(service))
    }

    fn active_state(&self, service: ManagedService) -> Result<String, InstallError> {
        Ok(if self.is_active(service)? {
            "active".into()
        } else {
            "inactive".into()
        })
    }

    fn start(&self, service: ManagedService) -> Result<(), InstallError> {
        run_rc_service(&self.rc_service, &["start", self.service_name(service)])
    }

    fn stop(&self, service: ManagedService) -> Result<(), InstallError> {
        run_rc_service(&self.rc_service, &["stop", self.service_name(service)])
    }

    fn restart(&self, service: ManagedService) -> Result<(), InstallError> {
        run_rc_service(&self.rc_service, &["restart", self.service_name(service)])
    }

    fn main_pid(&self, service: ManagedService) -> Result<u32, InstallError> {
        super::manager::pid_of_command(main_pid_pattern(service))
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

fn probe(openrc: &Openrc) -> Availability {
    if pid1_is_systemd() {
        return Availability::NotDetected;
    }
    if !openrc.init_d_dir.is_dir() {
        return Availability::NotDetected;
    }
    if !is_executable(&openrc.rc_service) || !is_executable(&openrc.rc_update) {
        return Availability::Unavailable(format!(
            "{} or {} is missing or not executable",
            openrc.rc_service.display(),
            openrc.rc_update.display()
        ));
    }
    if run_version(&openrc.rc_update, &["--version"]).is_err() {
        return Availability::Unavailable(format!(
            "{} does not answer",
            openrc.rc_update.display()
        ));
    }
    Availability::Available {
        version: "openrc".into(),
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

fn run_version(tool: &Path, args: &[&str]) -> Result<String, InstallError> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", tool.display())))?;
    if !output.status.success() {
        return Err(InstallError::Systemd(format!(
            "{} {} failed: {}",
            tool.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn render_router_script(canonical_root: &Path) -> String {
    format!(
        "#!/sbin/openrc-run\n\nname=\"Landscape Router\"\ndescription=\"Landscape webserver reverse proxy\"\n\ncommand=\"{}/current/landscape-webserver\"\ncommand_args=\"--config-dir {}/data --web {}/current/static\"\ncommand_user=\"root\"\n\nstart() {{\n    ebegin \"Starting Landscape\"\n    start-stop-daemon --start --make-pidfile --pidfile /run/landscape-router.pid --background --exec ${{command}} -- ${{command_args}}\n    eend $?\n}}\n\nstop() {{\n    ebegin \"Stopping Landscape\"\n    start-stop-daemon --stop --pidfile /run/landscape-router.pid\n    eend $?\n}}\n",
        shell_quote(&canonical_root.display().to_string()),
        shell_quote(&canonical_root.display().to_string()),
        shell_quote(&canonical_root.display().to_string()),
    )
}

fn render_lkit_script(binary: &Path) -> String {
    format!(
        "#!/sbin/openrc-run\n\nname=\"lkit\"\ndescription=\"lkit resident daemon\"\n\ncommand=\"{}\"\ncommand_args=\"daemon\"\ncommand_user=\"root\"\n\nstart() {{\n    ebegin \"Starting lkit daemon\"\n    start-stop-daemon --start --make-pidfile --pidfile /run/lkit.pid --background --exec ${{command}} -- ${{command_args}}\n    eend $?\n}}\n\nstop() {{\n    ebegin \"Stopping lkit daemon\"\n    start-stop-daemon --stop --pidfile /run/lkit.pid\n    eend $?\n}}\n",
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
    let expected_command = match service {
        ManagedService::LandscapeRouter => format!(
            "command=\"{}/current/landscape-webserver\"",
            shell_quote(&canonical_root.display().to_string())
        ),
        ManagedService::LkitDaemon => format!(
            "command=\"{}\"",
            shell_quote(&canonical_root.display().to_string())
        ),
    };
    if !content.contains(&expected_command) {
        return Err(InstallError::Systemd(format!(
            "service definition no longer points at the managed executable: expected {expected_command}"
        )));
    }
    if !content.contains("command_user=\"root\"") {
        return Err(InstallError::Systemd(
            "service definition must run as root".into(),
        ));
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

fn run_rc_service(rc_service: &Path, args: &[&str]) -> Result<(), InstallError> {
    let output = Command::new(rc_service)
        .args(args)
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", rc_service.display())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallError::Systemd(format!(
        "{} {} failed: {}",
        rc_service.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn rc_service_status(rc_service: &Path, name: &str) -> Result<bool, InstallError> {
    let output = Command::new(rc_service)
        .args(["status", name])
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", rc_service.display())))?;
    if output.status.success() {
        return Ok(true);
    }
    // rc-service status 约定:退出码 3 表示服务已停止,其他非零视为错误。
    if output.status.code() == Some(3) {
        return Ok(false);
    }
    Err(InstallError::Systemd(format!(
        "{} status {} failed: {}",
        rc_service.display(),
        name,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn run_rc_update(rc_update: &Path, args: &[&str]) -> Result<(), InstallError> {
    let output = Command::new(rc_update)
        .args(args)
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", rc_update.display())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(InstallError::Systemd(format!(
        "{} {} failed: {}",
        rc_update.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn rc_update_show(rc_update: &Path, name: &str) -> Result<bool, InstallError> {
    let output = Command::new(rc_update)
        .args(["show"])
        .output()
        .map_err(|error| InstallError::Systemd(format!("run {}: {error}", rc_update.display())))?;
    if !output.status.success() {
        return Err(InstallError::Systemd(format!(
            "{} show failed: {}",
            rc_update.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let table = String::from_utf8_lossy(&output.stdout);
    Ok(table
        .lines()
        .any(|line| line.split_whitespace().next() == Some(name)))
}

fn main_pid_pattern(service: ManagedService) -> &'static str {
    match service {
        ManagedService::LandscapeRouter => "current/landscape-webserver",
        ManagedService::LkitDaemon => "lkit daemon",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lkit-openrc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn renders_router_script_pointing_at_managed_executable() {
        let root = temp_dir("render");
        let script = render_router_script(&root);
        assert!(script.starts_with("#!/sbin/openrc-run"));
        assert!(script.contains(&format!("{}/current/landscape-webserver", root.display())));
        assert!(script.contains("command_user=\"root\""));
        validate_script(&script, ManagedService::LandscapeRouter, &root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renders_lkit_script_and_validates() {
        let binary = Path::new("/usr/local/bin/lkit");
        let script = render_lkit_script(binary);
        assert!(script.contains("command=\"/usr/local/bin/lkit\""));
        assert!(script.contains("command_args=\"daemon\""));
        assert!(!script.contains("/srv/landscape"));
        validate_script(&script, ManagedService::LkitDaemon, binary).unwrap();
    }

    #[test]
    fn validate_rejects_tampered_definition() {
        let root = temp_dir("validate");
        let mut script = render_router_script(&root);
        assert!(validate_script(&script, ManagedService::LandscapeRouter, &root).is_ok());
        script = script.replace(
            &format!("{}/current/landscape-webserver", root.display()),
            "/tmp/other-webserver",
        );
        assert!(validate_script(&script, ManagedService::LandscapeRouter, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registration_link_lifecycle() {
        let dir = temp_dir("registration");
        let origin = dir.join("service/landscape-router");
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::write(&origin, "#!/sbin/openrc-run\n").unwrap();
        let path = dir.join("init.d/landscape-router");
        assert_eq!(
            query_registration_at(&path).unwrap(),
            SystemRegistration::Missing
        );
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

    #[test]
    fn registration_conflict_is_reported() {
        let dir = temp_dir("conflict");
        let path = dir.join("init.d/landscape-router");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        assert!(matches!(
            query_registration_at(&path).unwrap(),
            SystemRegistration::Conflict { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
