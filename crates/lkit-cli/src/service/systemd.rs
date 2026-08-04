use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::InstallError;
use super::transaction::HostServiceBefore;
use super::transaction::{RegistrationKind, SystemdBefore};

pub(crate) const UNIT_NAME: &str = "landscape-router.service";
pub(crate) const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Systemd {
    pub systemctl: PathBuf,
    pub system_unit_dir: PathBuf,
    pub run_systemd_dir: PathBuf,
    pub pid1_is_systemd: bool,
    pub resolv_conf: PathBuf,
}

impl Systemd {
    pub(crate) fn host() -> Self {
        Self {
            systemctl: PathBuf::from("/bin/systemctl"),
            system_unit_dir: PathBuf::from(SYSTEM_UNIT_DIR),
            run_systemd_dir: PathBuf::from("/run/systemd/system"),
            pid1_is_systemd: pid1_is_systemd(),
            resolv_conf: PathBuf::from(super::resolv::RESOLV_CONF),
        }
    }

    pub(crate) fn probe(&self) -> Availability {
        probe(self, &self.run_systemd_dir, self.pid1_is_systemd)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Availability {
    /// 满足全部可用性条件。
    Available { version: String },
    /// PID 1 不是 systemd。
    NotSystemdInit,
    /// `systemctl` 缺失或不可执行。
    MissingSystemctl,
    /// 看似 systemd 但无法连接 manager,属于环境损坏。
    Unreachable(String),
}

/// 可用性判断。`run_systemd_dir` 注入用于测试;默认 `/run/systemd/system`。
pub(crate) fn probe(
    systemd: &Systemd,
    run_systemd_dir: &Path,
    pid1_is_systemd: bool,
) -> Availability {
    if !pid1_is_systemd {
        return Availability::NotSystemdInit;
    }
    if !run_systemd_dir.is_dir() {
        return Availability::NotSystemdInit;
    }
    if !systemd.systemctl.is_file() {
        return Availability::MissingSystemctl;
    }
    if !is_executable(&systemd.systemctl) {
        return Availability::MissingSystemctl;
    }
    match run_systemctl(systemd, &["show", "--property=Version"], None) {
        Ok((_, output)) => {
            let version = output
                .lines()
                .find_map(|line| line.strip_prefix("Version="))
                .unwrap_or_default()
                .to_string();
            Availability::Available { version }
        }
        Err(_) => Availability::Unreachable(
            "systemctl show --property=Version cannot connect to the systemd manager".into(),
        ),
    }
}

pub(crate) fn pid1_is_systemd() -> bool {
    std::fs::read_link("/proc/1/exe")
        .ok()
        .map(|path| path.file_name().is_some_and(|name| name == "systemd"))
        .unwrap_or(false)
}

/// 渲染受管 unit 原件内容。`ExecStart` 使用真实绝对路径。
pub(crate) fn render_unit(canonical_root: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Landscape Router\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={0}/current/landscape-webserver --config-dir {0}/data --web {0}/current/static\n\
         User=root\n\
         Restart=always\n\
         LimitMEMLOCK=infinity\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        canonical_root.display()
    )
}

/// 解析 unit 内容。返回按 section 分组的 `(key, value)` 列表。
/// 无法解析(非注释、非 section、非空且不含 `=` 的行)时返回错误。
fn parse_unit(content: &str) -> Result<Vec<(String, String)>, InstallError> {
    let mut entries = Vec::new();
    for (index, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(systemd_error(format!(
                "unit line {} is not parseable: {raw_line:?}",
                index + 1
            )));
        };
        entries.push((key.trim().to_string(), value.trim().to_string()));
    }
    Ok(entries)
}

/// 校验 unit 原件是否满足受管安全不变量:
/// - 可解析;
/// - `ExecStart` 恰好指向本安装根目录的 `current/landscape-webserver`,
///   且 `--config-dir` 和 `--web` 分别指向 `data` 与 `current/static`;
/// - `User=root`、`Restart=always`、`LimitMEMLOCK=infinity`、`WantedBy=multi-user.target`;
/// - 不包含明显的凭据内容。
pub(crate) fn validate_unit(content: &str, canonical_root: &Path) -> Result<(), InstallError> {
    let entries = parse_unit(content)?;
    let root = canonical_root.display();
    let expected_exec = format!(
        "{root}/current/landscape-webserver --config-dir {root}/data --web {root}/current/static"
    );
    let mut exec_start: Option<&str> = None;
    let mut user: Option<&str> = None;
    let mut restart: Option<&str> = None;
    let mut memlock: Option<&str> = None;
    let mut wanted_by: Option<&str> = None;
    for (key, value) in &entries {
        match key.as_str() {
            "ExecStart" => exec_start = Some(value),
            "User" => user = Some(value),
            "Restart" => restart = Some(value),
            "LimitMEMLOCK" => memlock = Some(value),
            "WantedBy" => wanted_by = Some(value),
            _ => {}
        }
        let lower = value.to_ascii_lowercase();
        if lower.contains("admin_pass")
            || lower.contains("admin_password")
            || lower.contains("landscape_admin")
            || lower.contains("landscape_password")
            || lower.contains("bearer ")
        {
            return Err(systemd_error(
                "unit must not contain credential material".into(),
            ));
        }
    }
    if exec_start != Some(expected_exec.as_str()) {
        return Err(systemd_error(
            "unit ExecStart must be the managed command for this install root".into(),
        ));
    }
    if user != Some("root") {
        return Err(systemd_error("unit User must be root".into()));
    }
    if restart != Some("always") {
        return Err(systemd_error("unit Restart must be always".into()));
    }
    if memlock != Some("infinity") {
        return Err(systemd_error("unit LimitMEMLOCK must be infinity".into()));
    }
    if wanted_by != Some("multi-user.target") {
        return Err(systemd_error(
            "unit WantedBy must be multi-user.target".into(),
        ));
    }
    Ok(())
}

/// 系统注册链接的状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Registration {
    /// 不存在注册链接。
    Missing,
    /// 指向受管原件的符号链接,记录原始目标。
    Symlink { target: PathBuf },
    /// 普通文件或其他无法证明归属的内容,属于所有权冲突。
    Conflict { file_type: String },
}

pub(crate) fn query_registration(systemd: &Systemd) -> Result<Registration, InstallError> {
    let path = systemd.system_unit_dir.join(UNIT_NAME);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registration::Missing),
        Err(error) => Err(InstallError::Io(error)),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&path).map_err(InstallError::Io)?;
                Ok(Registration::Symlink { target })
            } else {
                Ok(Registration::Conflict {
                    file_type: format!("{:?}", metadata.file_type()),
                })
            }
        }
    }
}

/// 创建系统注册链接(指向受管原件),使用临时链接加原子 rename。
/// 已存在相同目标链接时视为已注册;其他内容属于所有权冲突。
pub(crate) fn register(systemd: &Systemd, unit_origin: &Path) -> Result<(), InstallError> {
    let unit_origin = unit_origin.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(UNIT_NAME);
    match query_registration(systemd)? {
        Registration::Symlink { target } if target == unit_origin => {
            daemon_reload(systemd)?;
            return Ok(());
        }
        Registration::Missing => {}
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to overwrite",
                path.display()
            )));
        }
    }
    let tmp = systemd.system_unit_dir.join(format!(".{UNIT_NAME}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    symlink(&unit_origin, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })?;
    daemon_reload(systemd)
}

/// 移除系统注册链接(仅当它指向受管原件),并执行 daemon-reload。
pub(crate) fn unregister(systemd: &Systemd, unit_origin: &Path) -> Result<(), InstallError> {
    let unit_origin = unit_origin.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(UNIT_NAME);
    match query_registration(systemd)? {
        Registration::Symlink { target } if target == unit_origin => {
            std::fs::remove_file(&path).map_err(InstallError::Io)?;
        }
        Registration::Missing => return Ok(()),
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to remove",
                path.display()
            )));
        }
    }
    daemon_reload(systemd)
}

pub(crate) fn is_enabled(systemd: &Systemd) -> Result<bool, InstallError> {
    let output = systemctl_output(systemd, &["is-enabled", UNIT_NAME])?;
    let state = String::from_utf8_lossy(&output.stdout);
    match state.trim() {
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias" => Ok(true),
        "disabled" | "static" | "indirect" | "masked" | "masked-runtime" | "generated"
        | "transient" | "not-found" => Ok(false),
        // systemd 252 对缺失 unit 将错误写入 stderr 且以非零退出,
        // stdout 为空;管理器可达性由 probe 保证,这里表示未启用。
        "" => Ok(false),
        value => Err(query_error(
            systemd,
            &["is-enabled", UNIT_NAME],
            &output,
            value,
        )),
    }
}

pub(crate) fn is_active(systemd: &Systemd) -> Result<bool, InstallError> {
    let output = systemctl_output(systemd, &["is-active", UNIT_NAME])?;
    let state = String::from_utf8_lossy(&output.stdout);
    match state.trim() {
        "active" | "reloading" => Ok(true),
        "inactive" | "failed" | "activating" | "deactivating" | "maintenance" | "unknown" => {
            Ok(false)
        }
        value => Err(query_error(
            systemd,
            &["is-active", UNIT_NAME],
            &output,
            value,
        )),
    }
}

/// 查询 unit 的 ActiveState(`active`/`inactive`/`activating`/`failed`)。
pub(crate) fn active_state(systemd: &Systemd) -> Result<String, InstallError> {
    let (_, output) = run_systemctl(
        systemd,
        &["show", "--property=ActiveState", "--value", UNIT_NAME],
        None,
    )?;
    Ok(output.trim().to_string())
}

/// 查询 unit 的 MainPID(未运行时为 0)。
pub(crate) fn main_pid(systemd: &Systemd) -> Result<u32, InstallError> {
    let (_, output) = run_systemctl(
        systemd,
        &["show", "--property=MainPID", "--value", UNIT_NAME],
        None,
    )?;
    output
        .trim()
        .parse::<u32>()
        .map_err(|_| systemd_error(format!("invalid MainPID output {:?}", output.trim())))
}

pub(crate) fn enable(systemd: &Systemd) -> Result<(), InstallError> {
    run_systemctl(systemd, &["enable", UNIT_NAME], None).map(|_| ())
}

pub(crate) fn disable(systemd: &Systemd) -> Result<(), InstallError> {
    run_systemctl(systemd, &["disable", UNIT_NAME], None).map(|_| ())
}

pub(crate) fn start(systemd: &Systemd) -> Result<(), InstallError> {
    run_systemctl(systemd, &["start", UNIT_NAME], None).map(|_| ())
}

pub(crate) fn stop(systemd: &Systemd) -> Result<(), InstallError> {
    run_systemctl(systemd, &["stop", UNIT_NAME], None).map(|_| ())
}

pub(crate) fn daemon_reload(systemd: &Systemd) -> Result<(), InstallError> {
    run_systemctl(systemd, &["daemon-reload"], None).map(|_| ())
}

pub(crate) fn inspect_host_service(
    systemd: &Systemd,
    unit: &str,
) -> Result<HostServiceBefore, InstallError> {
    validate_unit_name(unit)?;
    let load = unit_property(systemd, unit, "LoadState")?;
    let installed = !matches!(load.as_str(), "not-found" | "error" | "");
    let active = installed && unit_query(systemd, "is-active", unit)? == "active";
    let enable_state = if installed {
        unit_query(systemd, "is-enabled", unit)?
    } else {
        "not-found".into()
    };
    Ok(HostServiceBefore {
        unit: unit.to_string(),
        installed,
        active,
        enable_state,
    })
}

pub(crate) fn stop_disable_mask_host_service(
    systemd: &Systemd,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    if !before.installed {
        return Ok(());
    }
    unit_command(systemd, "stop", &before.unit)?;
    if matches!(
        before.enable_state.as_str(),
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias"
    ) {
        unit_command(systemd, "disable", &before.unit)?;
    }
    unit_command(systemd, "mask", &before.unit)
}

pub(crate) fn restore_host_service(
    systemd: &Systemd,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    if !before.installed {
        return Ok(());
    }
    unit_command(systemd, "unmask", &before.unit)?;
    match before.enable_state.as_str() {
        "enabled" | "linked" | "alias" => unit_command(systemd, "enable", &before.unit)?,
        "enabled-runtime" | "linked-runtime" => {
            run_systemctl(systemd, &["enable", "--runtime", &before.unit], None)?;
        }
        "masked" | "masked-runtime" => unit_command(systemd, "mask", &before.unit)?,
        _ => {}
    }
    if before.active {
        unit_command(systemd, "start", &before.unit)?;
    } else {
        unit_command(systemd, "stop", &before.unit)?;
    }
    Ok(())
}

pub(crate) fn unit_command(systemd: &Systemd, verb: &str, unit: &str) -> Result<(), InstallError> {
    validate_unit_name(unit)?;
    run_systemctl(systemd, &[verb, unit], None).map(|_| ())
}

pub(crate) fn unit_query(
    systemd: &Systemd,
    verb: &str,
    unit: &str,
) -> Result<String, InstallError> {
    validate_unit_name(unit)?;
    let output = systemctl_output(systemd, &[verb, unit])?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !value.is_empty() {
        Ok(value)
    } else if output.status.success() {
        Ok("active".into())
    } else {
        Ok(if verb == "is-enabled" {
            "not-found".into()
        } else {
            "inactive".into()
        })
    }
}

pub(crate) fn unit_property(
    systemd: &Systemd,
    unit: &str,
    property: &str,
) -> Result<String, InstallError> {
    validate_unit_name(unit)?;
    if property.is_empty() || !property.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(systemd_error("invalid systemd property name".into()));
    }
    let argument = format!("--property={property}");
    let (_, output) = run_systemctl(systemd, &["show", &argument, "--value", unit], None)?;
    Ok(output.trim().to_string())
}

fn validate_unit_name(unit: &str) -> Result<(), InstallError> {
    if unit.is_empty()
        || unit.len() > 255
        || !unit.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.' | b'@')
        })
    {
        return Err(systemd_error(format!("invalid systemd unit name {unit:?}")));
    }
    Ok(())
}

/// 停止服务并确认受管进程退出。`wait_for_exit` 返回进程是否已退出,
/// 注入用于测试;默认实现检查 PID 1 的 unit 状态。
pub(crate) fn stop_and_wait(
    systemd: &Systemd,
    wait_for_exit: impl Fn() -> bool,
) -> Result<(), InstallError> {
    stop(systemd)?;
    for _ in 0..30 {
        if wait_for_exit() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(systemd_error(
        "unit did not exit within the timeout after stop".into(),
    ))
}

/// 恢复注册链接与 enabled/active 状态,并执行 daemon-reload。
/// 期望状态为「不存在」的恢复(未注册/未启用/未运行)采用宽容路径,
/// 已经处于期望状态时不视为失败;期望状态为「存在」的恢复失败则报错。
pub(crate) fn restore_systemd_before(
    systemd: &Systemd,
    before: &SystemdBefore,
    unit_origin: &Path,
) -> Result<(), InstallError> {
    match &before.registration.kind {
        RegistrationKind::Missing => unregister(systemd, unit_origin)?,
        RegistrationKind::Symlink => {
            if matches!(
                query_registration(systemd)?,
                Registration::Symlink { target }
                    if target == unit_origin.canonicalize().map_err(InstallError::Io)?
            ) {
                std::fs::remove_file(systemd.system_unit_dir.join(UNIT_NAME))
                    .map_err(InstallError::Io)?;
            }
            let target = before.registration.target.as_deref().ok_or_else(|| {
                systemd_error("symlink registration is missing its target".into())
            })?;
            restore_registration_link(systemd, Path::new(target))?;
        }
    }
    if before.enabled {
        enable(systemd)?;
    } else if is_enabled(systemd).unwrap_or(false) {
        disable(systemd)?;
    }
    if before.active {
        start(systemd)?;
    } else if is_active(systemd).unwrap_or(false) {
        stop(systemd)?;
    }
    daemon_reload(systemd)
}

/// 创建指向任意目标的注册链接(原子替换),所有权冲突时拒绝。
fn restore_registration_link(systemd: &Systemd, target: &Path) -> Result<(), InstallError> {
    let target = target.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(UNIT_NAME);
    match query_registration(systemd)? {
        Registration::Symlink { target: existing } if existing == target => return Ok(()),
        Registration::Missing => {}
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to overwrite",
                path.display()
            )));
        }
    }
    let tmp = systemd.system_unit_dir.join(format!(".{UNIT_NAME}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    symlink(&target, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })
}

fn run_systemctl(
    systemd: &Systemd,
    args: &[&str],
    _env: Option<&[(&str, &str)]>,
) -> Result<(std::process::ExitStatus, String), InstallError> {
    let output = systemctl_output(systemd, args)?;
    if output.status.success() {
        Ok((
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(systemd_error(format!(
            "{} {} failed with status {:?}: {}",
            systemd.systemctl.display(),
            args.join(" "),
            output.status.code(),
            stderr.trim()
        )))
    }
}

fn systemctl_output(
    systemd: &Systemd,
    args: &[&str],
) -> Result<std::process::Output, InstallError> {
    Command::new(&systemd.systemctl)
        .args(args)
        .output()
        .map_err(|error| {
            systemd_error(format!(
                "failed to run {}: {error}",
                systemd.systemctl.display()
            ))
        })
}

fn query_error(
    systemd: &Systemd,
    args: &[&str],
    output: &std::process::Output,
    value: &str,
) -> InstallError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    systemd_error(format!(
        "{} {} returned unknown state {value:?} with status {:?}: {}",
        systemd.systemctl.display(),
        args.join(" "),
        output.status.code(),
        stderr.trim()
    ))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn systemd_error(reason: String) -> InstallError {
    InstallError::Systemd(reason)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-systemd-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn fake_systemctl(dir: &Path, behavior: &str) -> PathBuf {
        let script = dir.join("systemctl");
        std::fs::write(&script, behavior).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn fake_systemd(dir: &Path) -> Systemd {
        Systemd {
            systemctl: dir.join("systemctl"),
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        }
    }

    #[test]
    fn probes_availability() {
        let dir = temp_dir("probe");
        let systemd = fake_systemd(&dir);
        let fake = fake_systemctl(
            &dir,
            "#!/bin/sh\ncase \"$*\" in\n  *--property=Version) echo \"Version=252.19\" ;;\n  *) exit 1 ;;\nesac\n",
        );
        std::fs::create_dir_all(dir.join("run")).unwrap();
        assert_eq!(
            probe(&systemd, &dir.join("run"), true),
            Availability::Available {
                version: "252.19".into()
            }
        );
        assert_eq!(
            probe(&systemd, &dir.join("run"), false),
            Availability::NotSystemdInit
        );
        assert_eq!(
            probe(&systemd, &dir.join("missing"), true),
            Availability::NotSystemdInit
        );
        std::fs::remove_file(&fake).unwrap();
        assert_eq!(
            probe(&systemd, &dir.join("run"), true),
            Availability::MissingSystemctl
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_and_validates_unit() {
        let root = Path::new("/srv/landscape");
        let content = render_unit(root);
        assert!(validate_unit(&content, root).is_ok());

        let tampered = content.replace("User=root", "User=landscape");
        assert!(validate_unit(&tampered, root).is_err());

        let other_root = content.replace("/srv/landscape", "/srv/other");
        assert!(validate_unit(&other_root, root).is_err());

        let secret = format!("{content}\nEnvironment=ADMIN_PASS=hunter2\n");
        assert!(validate_unit(&secret, root).is_err());

        assert!(validate_unit("[Unit]\nnot a key value line\n", root).is_err());
        assert!(validate_unit("", root).is_err());
    }

    #[test]
    fn registration_lifecycle() {
        let dir = temp_dir("reg");
        let systemd = fake_systemd(&dir);
        std::fs::create_dir_all(&systemd.system_unit_dir).unwrap();
        let origin = dir.join("service/landscape-router.service");
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::write(&origin, render_unit(Path::new("/srv/landscape"))).unwrap();
        let fake = fake_systemctl(&dir, "#!/bin/sh\nexit 0\n");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(query_registration(&systemd).unwrap(), Registration::Missing);
        register(&systemd, &origin).unwrap();
        assert_eq!(
            query_registration(&systemd).unwrap(),
            Registration::Symlink {
                target: origin.canonicalize().unwrap()
            }
        );
        register(&systemd, &origin).unwrap();

        let conflict = dir.join("units/landscape-router.service");
        std::fs::remove_file(&conflict).unwrap();
        std::fs::write(&conflict, "plain file\n").unwrap();
        assert!(matches!(
            query_registration(&systemd).unwrap(),
            Registration::Conflict { .. }
        ));
        assert!(register(&systemd, &origin).is_err());
        assert!(unregister(&systemd, &origin).is_err());

        std::fs::remove_file(&conflict).unwrap();
        register(&systemd, &origin).unwrap();
        unregister(&systemd, &origin).unwrap();
        assert_eq!(query_registration(&systemd).unwrap(), Registration::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queries_and_controls_state() {
        let dir = temp_dir("state");
        let systemd = fake_systemd(&dir);
        let behavior = r#"#!/bin/sh
case "$*" in
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") echo active;;
  "show --property=ActiveState --value landscape-router.service") echo active;;
  *) echo "fake called: $*" >&2; exit 1;;
esac
"#;
        fake_systemctl(&dir, behavior);
        assert!(is_enabled(&systemd).unwrap());
        assert!(is_active(&systemd).unwrap());
        assert_eq!(active_state(&systemd).unwrap(), "active");
        assert!(enable(&systemd).is_err());
        assert!(stop(&systemd).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepts_expected_negative_query_exit_codes() {
        let dir = temp_dir("negative-state");
        let systemd = fake_systemd(&dir);
        fake_systemctl(
            &dir,
            r#"#!/bin/sh
case "$*" in
  "is-enabled landscape-router.service") echo disabled; exit 1;;
  "is-active landscape-router.service") echo inactive; exit 3;;
  *) exit 1;;
esac
"#,
        );
        assert!(!is_enabled(&systemd).unwrap());
        assert!(!is_active(&systemd).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_waits_for_exit() {
        let dir = temp_dir("stop");
        let systemd = fake_systemd(&dir);
        fake_systemctl(&dir, "#!/bin/sh\nexit 0\n");
        if let Err(error) = stop_and_wait(&systemd, || true) {
            panic!("stop_and_wait failed: {error:?}");
        }
        assert!(matches!(
            stop_and_wait(&systemd, || false),
            Err(InstallError::Systemd(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
