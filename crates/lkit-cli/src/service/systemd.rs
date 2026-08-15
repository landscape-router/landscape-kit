use std::any::Any;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) use super::manager::{
    Availability, ManagedService, Registration, RegistrationKind, ServiceBefore, ServiceManager,
    ServiceManagerKind, SystemRegistration,
};
use super::plan::InstallError;
use super::transaction::HostServiceBefore;

pub(crate) const UNIT_NAME: &str = "landscape-router.service";
pub(crate) const LKIT_DAEMON_UNIT_NAME: &str = "lkit.service";
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
}

impl ServiceManager for Systemd {
    fn kind(&self) -> ServiceManagerKind {
        ServiceManagerKind::Systemd
    }

    fn probe(&self) -> Availability {
        probe(self, &self.run_systemd_dir, self.pid1_is_systemd)
    }

    fn service_name(&self, service: ManagedService) -> &str {
        match service {
            ManagedService::LandscapeRouter => UNIT_NAME,
            ManagedService::LkitDaemon => LKIT_DAEMON_UNIT_NAME,
        }
    }

    fn render_definition(
        &self,
        service: ManagedService,
        canonical_root: &Path,
    ) -> Result<String, InstallError> {
        match service {
            ManagedService::LandscapeRouter => Ok(render_unit(canonical_root)),
            ManagedService::LkitDaemon => Ok(render_lkit_daemon_unit(canonical_root)),
        }
    }

    fn validate_definition(
        &self,
        service: ManagedService,
        content: &str,
        canonical_root: &Path,
    ) -> Result<(), InstallError> {
        match service {
            ManagedService::LandscapeRouter => validate_unit(content, canonical_root),
            ManagedService::LkitDaemon => validate_lkit_daemon_unit(content, canonical_root),
        }
    }

    fn query_registration(
        &self,
        service: ManagedService,
    ) -> Result<SystemRegistration, InstallError> {
        query_registration_at(self, self.service_name(service))
    }

    fn register(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError> {
        register_at(self, self.service_name(service), origin)
    }

    fn unregister(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError> {
        unregister_at(self, self.service_name(service), origin)
    }

    fn is_enabled(&self, service: ManagedService) -> Result<bool, InstallError> {
        is_enabled_at(self, self.service_name(service))
    }

    fn enable(&self, service: ManagedService) -> Result<(), InstallError> {
        enable_at(self, self.service_name(service))
    }

    fn disable(&self, service: ManagedService) -> Result<(), InstallError> {
        disable_at(self, self.service_name(service))
    }

    fn is_active(&self, service: ManagedService) -> Result<bool, InstallError> {
        is_active_at(self, self.service_name(service))
    }

    fn active_state(&self, service: ManagedService) -> Result<String, InstallError> {
        active_state_at(self, self.service_name(service))
    }

    fn start(&self, service: ManagedService) -> Result<(), InstallError> {
        start_at(self, self.service_name(service))
    }

    fn stop(&self, service: ManagedService) -> Result<(), InstallError> {
        stop_at(self, self.service_name(service))
    }

    fn restart(&self, service: ManagedService) -> Result<(), InstallError> {
        unit_command(self, "restart", self.service_name(service))
    }

    fn refresh(&self) -> Result<(), InstallError> {
        daemon_reload(self)
    }

    fn main_pid(&self, service: ManagedService) -> Result<u32, InstallError> {
        main_pid_at(self, self.service_name(service))
    }

    fn restore_registration(
        &self,
        service: ManagedService,
        before: &ServiceBefore,
        origin: &Path,
    ) -> Result<(), InstallError> {
        restore_registration_at(self, self.service_name(service), before, origin)
    }

    fn resolv_conf(&self) -> &Path {
        &self.resolv_conf
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 从 trait 对象向下转型到 systemd 后端。只用于显式要求 systemd 的操作
/// (network takeover、旧部署迁移),调用方必须先通过 probe 保证可用。
pub(crate) fn downcast(manager: &dyn ServiceManager) -> Result<&Systemd, InstallError> {
    manager
        .as_any()
        .downcast_ref::<Systemd>()
        .ok_or_else(|| systemd_error("operation requires the systemd service manager".into()))
}

/// 可用性判断。`run_systemd_dir` 注入用于测试;默认 `/run/systemd/system`。
pub(crate) fn probe(
    systemd: &Systemd,
    run_systemd_dir: &Path,
    pid1_is_systemd: bool,
) -> Availability {
    if !pid1_is_systemd {
        return Availability::NotDetected;
    }
    if !run_systemd_dir.is_dir() {
        return Availability::NotDetected;
    }
    if !systemd.systemctl.is_file() {
        return Availability::Unavailable(
            "the systemctl binary is missing on a host running systemd".into(),
        );
    }
    if !is_executable(&systemd.systemctl) {
        return Availability::Unavailable(
            "the systemctl binary is not executable on a host running systemd".into(),
        );
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
        Err(_) => Availability::Unavailable(
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

/// 渲染受管 Landscape unit 原件内容。`ExecStart` 使用真实绝对路径。
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

/// 渲染 lkit 常驻服务 unit 原件内容。二进制约定为
/// `<install-root>/service/lkit`(与网络接管恢复二进制同目录约定)。
pub(crate) fn render_lkit_daemon_unit(canonical_root: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Lkit daemon\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={0}/service/lkit daemon --config-dir {0}/data\n\
         User=root\n\
         Restart=always\n\
         KillMode=process\n\
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
/// - `ExecStart` 恰好指向本安装根目录的受管命令;
/// - `User=root`、`Restart=always`、`WantedBy=multi-user.target`;
/// - Landscape 额外要求 `LimitMEMLOCK=infinity`;
/// - daemon 额外要求 `KillMode=process`(停服时只向主进程发信号,daemon
///   能完成进行中的委托请求,不会通过 cgroup 信号杀死执行子进程);
/// - 不包含明显的凭据内容。
pub(crate) fn validate_unit(content: &str, canonical_root: &Path) -> Result<(), InstallError> {
    let expected = format!(
        "{0}/current/landscape-webserver --config-dir {0}/data --web {0}/current/static",
        canonical_root.display()
    );
    validate_definition(content, expected, true, false)
}

fn validate_lkit_daemon_unit(content: &str, canonical_root: &Path) -> Result<(), InstallError> {
    let expected = format!(
        "{0}/service/lkit daemon --config-dir {0}/data",
        canonical_root.display()
    );
    validate_definition(content, expected, false, true)
}

fn validate_definition(
    content: &str,
    expected_exec: String,
    require_memlock: bool,
    require_kill_mode_process: bool,
) -> Result<(), InstallError> {
    let entries = parse_unit(content)?;
    let mut exec_start: Option<&str> = None;
    let mut user: Option<&str> = None;
    let mut restart: Option<&str> = None;
    let mut memlock: Option<&str> = None;
    let mut wanted_by: Option<&str> = None;
    let mut kill_mode: Option<&str> = None;
    for (key, value) in &entries {
        match key.as_str() {
            "ExecStart" => exec_start = Some(value),
            "User" => user = Some(value),
            "Restart" => restart = Some(value),
            "LimitMEMLOCK" => memlock = Some(value),
            "WantedBy" => wanted_by = Some(value),
            "KillMode" => kill_mode = Some(value),
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
    if require_memlock && memlock != Some("infinity") {
        return Err(systemd_error("unit LimitMEMLOCK must be infinity".into()));
    }
    if require_kill_mode_process && kill_mode != Some("process") {
        return Err(systemd_error("unit KillMode must be process".into()));
    }
    if wanted_by != Some("multi-user.target") {
        return Err(systemd_error(
            "unit WantedBy must be multi-user.target".into(),
        ));
    }
    Ok(())
}

fn query_registration_at(
    systemd: &Systemd,
    unit: &str,
) -> Result<SystemRegistration, InstallError> {
    let path = systemd.system_unit_dir.join(unit);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SystemRegistration::Missing)
        }
        Err(error) => Err(InstallError::Io(error)),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&path).map_err(InstallError::Io)?;
                Ok(SystemRegistration::Symlink { target })
            } else {
                Ok(SystemRegistration::Conflict {
                    file_type: format!("{:?}", metadata.file_type()),
                })
            }
        }
    }
}

/// 创建系统注册链接(指向受管原件),使用临时链接加原子 rename。
/// 已存在相同目标链接时视为已注册;其他内容属于所有权冲突。
fn register_at(systemd: &Systemd, unit: &str, unit_origin: &Path) -> Result<(), InstallError> {
    let unit_origin = unit_origin.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(unit);
    match query_registration_at(systemd, unit)? {
        SystemRegistration::Symlink { target } if target == unit_origin => {
            daemon_reload(systemd)?;
            return Ok(());
        }
        SystemRegistration::Missing => {}
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to overwrite",
                path.display()
            )));
        }
    }
    let tmp = systemd.system_unit_dir.join(format!(".{unit}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    symlink(&unit_origin, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })?;
    daemon_reload(systemd)
}

/// 移除系统注册链接(仅当它指向受管原件),并执行 daemon-reload。
fn unregister_at(systemd: &Systemd, unit: &str, unit_origin: &Path) -> Result<(), InstallError> {
    let unit_origin = unit_origin.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(unit);
    match query_registration_at(systemd, unit)? {
        SystemRegistration::Symlink { target } if target == unit_origin => {
            std::fs::remove_file(&path).map_err(InstallError::Io)?;
        }
        SystemRegistration::Missing => return Ok(()),
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to remove",
                path.display()
            )));
        }
    }
    daemon_reload(systemd)
}

fn is_enabled_at(systemd: &Systemd, unit: &str) -> Result<bool, InstallError> {
    let output = systemctl_output(systemd, &["is-enabled", unit])?;
    let state = String::from_utf8_lossy(&output.stdout);
    match state.trim() {
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias" => Ok(true),
        "disabled" | "static" | "indirect" | "masked" | "masked-runtime" | "generated"
        | "transient" | "not-found" => Ok(false),
        // systemd 252 对缺失 unit 将错误写入 stderr 且以非零退出,
        // stdout 为空;管理器可达性由 probe 保证,这里表示未启用。
        "" => Ok(false),
        value => Err(query_error(systemd, &["is-enabled", unit], &output, value)),
    }
}

fn is_active_at(systemd: &Systemd, unit: &str) -> Result<bool, InstallError> {
    let output = systemctl_output(systemd, &["is-active", unit])?;
    let state = String::from_utf8_lossy(&output.stdout);
    match state.trim() {
        "active" | "reloading" => Ok(true),
        "inactive" | "failed" | "activating" | "deactivating" | "maintenance" | "unknown" => {
            Ok(false)
        }
        value => Err(query_error(systemd, &["is-active", unit], &output, value)),
    }
}

/// 查询 unit 的 ActiveState(`active`/`inactive`/`activating`/`failed`)。
fn active_state_at(systemd: &Systemd, unit: &str) -> Result<String, InstallError> {
    let (_, output) = run_systemctl(
        systemd,
        &["show", "--property=ActiveState", "--value", unit],
        None,
    )?;
    Ok(output.trim().to_string())
}

/// 查询 unit 的 MainPID(未运行时为 0)。
fn main_pid_at(systemd: &Systemd, unit: &str) -> Result<u32, InstallError> {
    let (_, output) = run_systemctl(
        systemd,
        &["show", "--property=MainPID", "--value", unit],
        None,
    )?;
    output
        .trim()
        .parse::<u32>()
        .map_err(|_| systemd_error(format!("invalid MainPID output {:?}", output.trim())))
}

fn enable_at(systemd: &Systemd, unit: &str) -> Result<(), InstallError> {
    run_systemctl(systemd, &["enable", unit], None).map(|_| ())
}

fn disable_at(systemd: &Systemd, unit: &str) -> Result<(), InstallError> {
    run_systemctl(systemd, &["disable", unit], None).map(|_| ())
}

fn start_at(systemd: &Systemd, unit: &str) -> Result<(), InstallError> {
    run_systemctl(systemd, &["start", unit], None).map(|_| ())
}

fn stop_at(systemd: &Systemd, unit: &str) -> Result<(), InstallError> {
    run_systemctl(systemd, &["stop", unit], None).map(|_| ())
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

/// 从 ExecStart 值中提取 `--config-dir` 指向的目录,支持 `--config-dir <path>`
/// 与 `--config-dir=<path>` 两种形式,`<path>` 按 systemd 双引号/反斜杠规则解析。
fn exec_config_dir(exec_start: &str) -> Option<PathBuf> {
    let tokens = exec_tokens(exec_start);
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index] == "--config-dir" {
            if let Some(path) = tokens.get(index + 1) {
                return Some(PathBuf::from(path));
            }
        } else if let Some(path) = tokens[index].strip_prefix("--config-dir=")
            && !path.is_empty()
        {
            return Some(PathBuf::from(path));
        }
        index += 1;
    }
    None
}

/// 宽容的 systemd ExecStart 分词:支持双引号字符串与反斜杠转义,
/// 其余空白作为分隔符。不实现完整的 systemd 引号规则,只用于定位参数。
fn exec_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                in_token = true;
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            if let Some(next) = chars.next() {
                                current.push(next);
                            }
                        }
                        '"' => break,
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_token = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            other if other.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            other => {
                in_token = true;
                current.push(other);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

/// 在系统 unit 目录中查找 ExecStart 携带 `--config-dir <dir>` 的 service unit。
/// 用于发现指向旧手工部署 config 目录的旧 Landscape unit。
pub(crate) fn find_units_serving_config_dir(
    systemd: &Systemd,
    config_dir: &Path,
) -> Result<Vec<String>, InstallError> {
    let config_dir = config_dir.canonicalize().map_err(InstallError::Io)?;
    let mut dirs = vec![systemd.system_unit_dir.clone()];
    for dir in [
        "/etc/systemd/system",
        "/usr/lib/systemd/system",
        "/run/systemd/system",
    ] {
        let path = PathBuf::from(dir);
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    let mut found = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".service") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(entries) = parse_unit(&content) else {
                continue;
            };
            let Some(exec_start) = entries
                .iter()
                .find(|(key, _)| key == "ExecStart")
                .map(|(_, value)| value)
            else {
                continue;
            };
            let Some(served) = exec_config_dir(exec_start) else {
                continue;
            };
            let served = served.canonicalize().unwrap_or(served);
            if served == config_dir {
                found.push(name);
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// 查询 unit 原件的绝对路径(`systemctl show --property=FragmentPath`)。
pub(crate) fn fragment_path(systemd: &Systemd, unit: &str) -> Result<String, InstallError> {
    let value = unit_property(systemd, unit, "FragmentPath")?;
    if value.is_empty() {
        return Err(systemd_error(format!(
            "unit {unit} reports an empty FragmentPath"
        )));
    }
    Ok(value)
}

/// 恢复注册链接与 enabled/active 状态,并执行 daemon-reload。
/// 期望状态为「不存在」的恢复(未注册/未启用/未运行)采用宽容路径,
/// 已经处于期望状态时不视为失败;期望状态为「存在」的恢复失败则报错。
fn restore_registration_at(
    systemd: &Systemd,
    unit: &str,
    before: &ServiceBefore,
    unit_origin: &Path,
) -> Result<(), InstallError> {
    match &before.registration.kind {
        RegistrationKind::Missing => unregister_at(systemd, unit, unit_origin)?,
        RegistrationKind::Symlink => {
            if matches!(
                query_registration_at(systemd, unit)?,
                SystemRegistration::Symlink { target }
                    if target == unit_origin.canonicalize().map_err(InstallError::Io)?
            ) {
                std::fs::remove_file(systemd.system_unit_dir.join(unit))
                    .map_err(InstallError::Io)?;
            }
            let target = before.registration.target.as_deref().ok_or_else(|| {
                systemd_error("symlink registration is missing its target".into())
            })?;
            restore_registration_link_at(systemd, unit, Path::new(target))?;
        }
    }
    if before.enabled {
        enable_at(systemd, unit)?;
    } else if is_enabled_at(systemd, unit).unwrap_or(false) {
        disable_at(systemd, unit)?;
    }
    daemon_reload(systemd)
}

/// 创建指向任意目标的注册链接(原子替换),所有权冲突时拒绝。
fn restore_registration_link_at(
    systemd: &Systemd,
    unit: &str,
    target: &Path,
) -> Result<(), InstallError> {
    let target = target.canonicalize().map_err(InstallError::Io)?;
    let path = systemd.system_unit_dir.join(unit);
    match query_registration_at(systemd, unit)? {
        SystemRegistration::Symlink { target: existing } if existing == target => return Ok(()),
        SystemRegistration::Missing => {}
        _ => {
            return Err(systemd_error(format!(
                "{} is an ownership conflict; refusing to overwrite",
                path.display()
            )));
        }
    }
    let tmp = systemd.system_unit_dir.join(format!(".{unit}.tmp"));
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
            Availability::NotDetected
        );
        assert_eq!(
            probe(&systemd, &dir.join("missing"), true),
            Availability::NotDetected
        );
        std::fs::remove_file(&fake).unwrap();
        assert!(matches!(
            probe(&systemd, &dir.join("run"), true),
            Availability::Unavailable(_)
        ));
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
    fn renders_and_validates_lkit_daemon_unit() {
        let root = Path::new("/srv/landscape");
        let content = render_lkit_daemon_unit(root);
        assert!(validate_lkit_daemon_unit(&content, root).is_ok());

        let tampered = content.replace("/srv/landscape", "/srv/other");
        assert!(validate_lkit_daemon_unit(&tampered, root).is_err());

        let without_kill_mode = content.replace("KillMode=process\n", "");
        assert!(validate_lkit_daemon_unit(&without_kill_mode, root).is_err());

        assert!(validate_unit(&content, root).is_err());
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
        let service = ManagedService::LandscapeRouter;

        assert_eq!(
            systemd.query_registration(service).unwrap(),
            SystemRegistration::Missing
        );
        systemd.register(service, &origin).unwrap();
        assert_eq!(
            systemd.query_registration(service).unwrap(),
            SystemRegistration::Symlink {
                target: origin.canonicalize().unwrap()
            }
        );
        systemd.register(service, &origin).unwrap();

        let conflict = dir.join("units/landscape-router.service");
        std::fs::remove_file(&conflict).unwrap();
        std::fs::write(&conflict, "plain file\n").unwrap();
        assert!(matches!(
            systemd.query_registration(service).unwrap(),
            SystemRegistration::Conflict { .. }
        ));
        assert!(systemd.register(service, &origin).is_err());
        assert!(systemd.unregister(service, &origin).is_err());

        std::fs::remove_file(&conflict).unwrap();
        systemd.register(service, &origin).unwrap();
        systemd.unregister(service, &origin).unwrap();
        assert_eq!(
            systemd.query_registration(service).unwrap(),
            SystemRegistration::Missing
        );
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
        let service = ManagedService::LandscapeRouter;
        assert!(systemd.is_enabled(service).unwrap());
        assert!(systemd.is_active(service).unwrap());
        assert_eq!(systemd.active_state(service).unwrap(), "active");
        assert!(systemd.enable(service).is_err());
        assert!(systemd.stop(service).is_err());
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
        let service = ManagedService::LandscapeRouter;
        assert!(!systemd.is_enabled(service).unwrap());
        assert!(!systemd.is_active(service).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_waits_for_exit() {
        let dir = temp_dir("stop");
        let systemd = fake_systemd(&dir);
        fake_systemctl(&dir, "#!/bin/sh\nexit 0\n");
        let service = ManagedService::LandscapeRouter;
        if let Err(error) = systemd.stop_and_wait(service, &(|| true)) {
            panic!("stop_and_wait failed: {error:?}");
        }
        assert!(matches!(
            systemd.stop_and_wait(service, &(|| false)),
            Err(InstallError::Systemd(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
