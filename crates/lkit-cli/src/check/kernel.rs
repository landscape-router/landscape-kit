use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use super::model::{CheckResult, Status};

pub fn run() -> Vec<CheckResult> {
    vec![
        kernel_version(),
        bpf(),
        btf(),
        cgroup(),
        cgroup_cpu(),
        cgroup_bpf(),
        bpf_events(),
    ]
}

fn kernel_version() -> CheckResult {
    let mut result = CheckResult::new(
        "kernel.version",
        crate::tr!(crate::keys::KERNEL_KERNEL_VERSION),
    );
    match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(raw) => {
            let version = raw.trim().to_string();
            result.value = version.clone();
            match parse_version(&version) {
                Some((major, minor, _)) if (major, minor) >= (6, 9) => result.set(
                    Status::Pass,
                    version,
                    crate::tr!(crate::keys::KERNEL_KERNEL_MEETS_6_9_REQUIREMENT),
                ),
                Some((_, _, _)) => result
                    .set(
                        Status::Error,
                        version.clone(),
                        crate::tr!(
                            crate::keys::KERNEL_KERNEL_VERSION_BELOW_REQUIREMENT,
                            version = version
                        ),
                    )
                    .suggestion(crate::tr!(crate::keys::KERNEL_UPGRADE_KERNEL_6_9_OR_LATER)),
                None => result.set(
                    Status::Unknown,
                    version,
                    crate::tr!(crate::keys::KERNEL_UNABLE_PARSE_KERNEL_VERSION),
                ),
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_UNAVAILABLE),
            crate::tr!(crate::keys::KERNEL_UNABLE_READ_OSRELEASE, err = err),
        ),
    }
}

fn parse_version(value: &str) -> Option<(u32, u32, u32)> {
    let core = value.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

const BPF_PROG_GET_NEXT_ID: libc::c_int = 11;

#[repr(C)]
struct BpfProgGetNextIdAttr {
    start_id: u32,
    next_id: u32,
    open_flags: u32,
}

fn bpf() -> CheckResult {
    let mut result = CheckResult::new(
        "kernel.bpf",
        crate::tr!(crate::keys::KERNEL_BPF_SUBSYSTEM_AND_JIT),
    );
    #[cfg(target_os = "linux")]
    {
        let mut attr = BpfProgGetNextIdAttr {
            start_id: 0,
            next_id: 0,
            open_flags: 0,
        };
        let ret = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                BPF_PROG_GET_NEXT_ID,
                &mut attr as *mut BpfProgGetNextIdAttr as *mut libc::c_void,
                std::mem::size_of::<BpfProgGetNextIdAttr>() as u32,
            )
        };
        if ret == 0 {
            result = result.detail(crate::tr!(
                crate::keys::KERNEL_BPF_PROG_GET_NEXT_ID_PROBE_SUCCEEDED
            ));
        } else {
            let errno = std::io::Error::last_os_error();
            let is_root = unsafe { libc::geteuid() == 0 };
            match errno.raw_os_error() {
                Some(libc::ENOENT) => {
                    result =
                        result.detail(crate::tr!(crate::keys::KERNEL_BPF_PROG_GET_NEXT_ID_ENOENT));
                }
                Some(libc::ENOSYS) => {
                    return result
                        .set(
                            Status::Error,
                            crate::tr!(crate::keys::KERNEL_BPF_SYSCALL_UNAVAILABLE),
                            crate::tr!(crate::keys::KERNEL_KERNEL_DOES_NOT_SUPPORT_BPF_SYSCALL),
                        )
                        .suggestion(crate::tr!(crate::keys::KERNEL_USE_KERNEL_SUPPORTING_EBPF));
                }
                Some(libc::EPERM) | Some(libc::EACCES) if is_root => {
                    return result
                        .set(
                            Status::Error,
                            crate::tr!(crate::keys::KERNEL_BPF_DENIED),
                            crate::tr!(crate::keys::KERNEL_BPF_PERMISSION_ERROR_AS_ROOT),
                        )
                        .suggestion(crate::tr!(
                            crate::keys::KERNEL_CHECK_SECCOMP_OR_LSM_RESTRICTION
                        ));
                }
                Some(libc::EPERM) | Some(libc::EACCES) => {
                    return result
                        .set(
                            Status::Unknown,
                            crate::tr!(crate::keys::KERNEL_PERMISSION_DENIED),
                            crate::tr!(crate::keys::KERNEL_CANNOT_PROBE_BPF_WITH_CURRENT_IDENTITY),
                        )
                        .suggestion(crate::tr!(crate::keys::KERNEL_RUN_LKIT_CHECK_AS_ROOT));
                }
                Some(other) => {
                    return result.set(
                        Status::Unknown,
                        format!("errno {other}"),
                        crate::tr!(
                            crate::keys::KERNEL_BPF_PROBE_UNEXPECTED_ERROR,
                            errno = errno
                        ),
                    );
                }
                None => {
                    return result.set(
                        Status::Unknown,
                        crate::tr!(crate::keys::KERNEL_UNKNOWN_ERROR),
                        crate::tr!(crate::keys::KERNEL_BPF_PROBE_FAILED, errno = errno),
                    );
                }
            }
        }

        match std::fs::read_to_string("/proc/sys/net/core/bpf_jit_enable") {
            Ok(raw) => {
                let value = raw.trim().to_string();
                match value.as_str() {
                    "1" | "2" => result.set(
                        Status::Pass,
                        crate::tr!(crate::keys::KERNEL_BPF_AVAILABLE_JIT_ENABLED, value = value),
                        crate::tr!(crate::keys::KERNEL_BPF_SYSCALL_AND_JIT_AVAILABLE),
                    ),
                    "0" => result
                        .set(
                            Status::Error,
                            crate::tr!(crate::keys::KERNEL_JIT_DISABLED, value = value),
                            crate::tr!(crate::keys::KERNEL_BPF_JIT_IS_DISABLED),
                        )
                        .suggestion(crate::tr!(crate::keys::KERNEL_ENABLE_JIT_SYSCTL)),
                    _ => result.set(
                        Status::Unknown,
                        value,
                        crate::tr!(crate::keys::KERNEL_UNRECOGNIZED_BPF_JIT_STATUS),
                    ),
                }
            }
            Err(err) => match config_value("CONFIG_BPF_JIT") {
                Some('y') => result.set(
                    Status::Pass,
                    crate::tr!(crate::keys::KERNEL_JIT_STATUS_FILE_UNREADABLE_BUILTIN),
                    crate::tr!(crate::keys::KERNEL_CONFIG_BPF_JIT_Y, err = err),
                ),
                Some('m') => result.set(
                    Status::Unknown,
                    crate::tr!(crate::keys::KERNEL_JIT_IS_A_MODULE),
                    crate::tr!(crate::keys::KERNEL_UNABLE_CONFIRM_BPF_JIT_MODULE_LOADED),
                ),
                Some('n') => result
                    .set(
                        Status::Error,
                        "CONFIG_BPF_JIT=n",
                        crate::tr!(crate::keys::KERNEL_BPF_JIT_DISABLED_AT_BUILD),
                    )
                    .suggestion(crate::tr!(
                        crate::keys::KERNEL_USE_KERNEL_WITH_CONFIG_BPF_JIT
                    )),
                _ => result.set(
                    Status::Unknown,
                    crate::tr!(crate::keys::KERNEL_UNKNOWN),
                    crate::tr!(
                        crate::keys::KERNEL_UNABLE_READ_BPF_JIT_ENABLE_OR_CONFIG,
                        err = err
                    ),
                ),
            },
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_NOT_LINUX),
            crate::tr!(crate::keys::KERNEL_CURRENT_PLATFORM_CANNOT_PROBE_BPF),
        )
    }
}

fn btf() -> CheckResult {
    let result = CheckResult::new(
        "kernel.btf",
        crate::tr!(crate::keys::KERNEL_KERNEL_BTF_INFORMATION),
    );
    let path = "/sys/kernel/btf/vmlinux";
    match std::fs::File::open(path) {
        Ok(_) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::KERNEL_PRESENT_AND_READABLE),
            crate::tr!(crate::keys::KERNEL_KERNEL_BTF_INFORMATION_AVAILABLE),
        ),
        Err(err) if Path::new(path).exists() => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_PRESENT_BUT_UNREADABLE),
            crate::tr!(
                crate::keys::KERNEL_BTF_EXISTS_BUT_CANNOT_READ,
                path = path,
                err = err
            ),
        ),
        Err(_) => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::KERNEL_MISSING),
                crate::tr!(crate::keys::KERNEL_BTF_PATH_DOES_NOT_EXIST, path = path),
            )
            .suggestion(crate::tr!(crate::keys::KERNEL_USE_KERNEL_WITH_BTF_SUPPORT)),
    }
}

fn cgroup() -> CheckResult {
    let result = CheckResult::new(
        "kernel.cgroup",
        crate::tr!(crate::keys::KERNEL_CGROUP_FILESYSTEM),
    );
    match std::fs::read_to_string("/proc/self/mounts") {
        Ok(mounts) => {
            let mut found = None;
            for line in mounts.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3
                    && parts[1] == "/sys/fs/cgroup"
                    && (parts[2] == "cgroup2" || parts[2] == "cgroup")
                {
                    found = Some(parts[2]);
                    break;
                }
            }
            match found {
                Some(fstype) => {
                    if std::fs::read_dir("/sys/fs/cgroup").is_ok() {
                        result.set(
                            Status::Pass,
                            crate::tr!(crate::keys::KERNEL_MOUNTED, fstype = fstype),
                            crate::tr!(crate::keys::KERNEL_CGROUP_FILESYSTEM_AVAILABLE),
                        )
                    } else {
                        result.set(
                            Status::Unknown,
                            crate::tr!(crate::keys::KERNEL_MOUNTED_BUT_UNREADABLE, fstype = fstype),
                            crate::tr!(crate::keys::KERNEL_CGROUP_FS_UNREADABLE),
                        )
                    }
                }
                None => result
                    .set(
                        Status::Error,
                        crate::tr!(crate::keys::KERNEL_NOT_MOUNTED),
                        crate::tr!(crate::keys::KERNEL_NO_CGROUP_MOUNTED_AT_SYS_FS_CGROUP),
                    )
                    .suggestion(crate::tr!(crate::keys::KERNEL_CHECK_CGROUP_SUPPORT_ENABLED)),
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_UNAVAILABLE),
            crate::tr!(crate::keys::KERNEL_UNABLE_READ_SELF_MOUNTS, err = err),
        ),
    }
}

fn cgroup_cpu() -> CheckResult {
    let result = CheckResult::new("kernel.cgroup_cpu", "Cgroup CPU controller");
    match std::fs::read_to_string("/sys/fs/cgroup/cgroup.controllers") {
        Ok(controllers) => {
            if controllers.split_whitespace().any(|c| c == "cpu") {
                result.set(
                    Status::Pass,
                    crate::tr!(crate::keys::KERNEL_AVAILABLE),
                    crate::tr!(crate::keys::KERNEL_CGROUP_V2_CPU_CONTROLLER_ENABLED),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        crate::tr!(crate::keys::KERNEL_DISABLED),
                        crate::tr!(crate::keys::KERNEL_CGROUP_V2_CPU_CONTROLLER_NOT_AVAILABLE),
                    )
                    .suggestion(crate::tr!(crate::keys::KERNEL_ADD_CPU_TO_CONTROLLERS))
            }
        }
        Err(_) => {
            if Path::new("/sys/fs/cgroup/cpu").is_dir() {
                result.set(
                    Status::Pass,
                    crate::tr!(crate::keys::KERNEL_AVAILABLE),
                    crate::tr!(crate::keys::KERNEL_CGROUP_V1_CPU_CONTROLLER_MOUNTED),
                )
            } else if Path::new("/sys/fs/cgroup").exists() {
                result.set(
                    Status::Unknown,
                    crate::tr!(crate::keys::KERNEL_UNKNOWN),
                    crate::tr!(crate::keys::KERNEL_UNABLE_READ_CGROUP_CONTROLLER_INFORMATION),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        crate::tr!(crate::keys::KERNEL_NOT_MOUNTED),
                        crate::tr!(crate::keys::KERNEL_CGROUP_FILESYSTEM_UNAVAILABLE),
                    )
                    .suggestion(crate::tr!(crate::keys::KERNEL_CHECK_CGROUP_SUPPORT_ENABLED))
            }
        }
    }
}

fn cgroup_bpf() -> CheckResult {
    config_check(
        "kernel.cgroup_bpf",
        crate::tr!(crate::keys::KERNEL_CGROUP_BPF_SUPPORT),
        "CONFIG_CGROUP_BPF",
    )
}

fn bpf_events() -> CheckResult {
    config_check(
        "kernel.bpf_events",
        crate::tr!(crate::keys::KERNEL_BPF_EVENTS_SUPPORT),
        "CONFIG_BPF_EVENTS",
    )
}

fn config_check(id: &'static str, title: impl Into<String>, name: &str) -> CheckResult {
    let result = CheckResult::new(id, title);
    match config_value(name) {
        Some('y') => result.set(
            Status::Pass,
            crate::tr!(crate::keys::KERNEL_ENABLED),
            crate::tr!(crate::keys::KERNEL_CONFIG_NAME_BUILTIN, name = name),
        ),
        Some('m') => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_MODULE),
            crate::tr!(crate::keys::KERNEL_CONFIG_NAME_MODULE, name = name),
        ),
        Some('n') => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::KERNEL_DISABLED),
                format!("{name}=n"),
            )
            .suggestion(crate::tr!(
                crate::keys::KERNEL_USE_KERNEL_WITH_CONFIG_OPTION
            )),
        _ => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::KERNEL_UNKNOWN),
            crate::tr!(crate::keys::KERNEL_UNABLE_READ_KERNEL_CONFIG_CAPABILITY),
        ),
    }
}

fn config_value(name: &str) -> Option<char> {
    config_map().as_ref().and_then(|map| map.get(name).copied())
}

fn config_map() -> &'static Option<HashMap<String, char>> {
    static CACHE: OnceLock<Option<HashMap<String, char>>> = OnceLock::new();
    CACHE.get_or_init(|| load_config().map(|raw| parse_config(&raw)))
}

fn load_config() -> Option<String> {
    if Path::new("/proc/config.gz").exists()
        && let Ok(output) = Command::new("zcat").arg("/proc/config.gz").output()
        && output.status.success()
        && let Ok(raw) = String::from_utf8(output.stdout)
    {
        return Some(raw);
    }
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()?
        .trim()
        .to_string();
    for path in [
        format!("/boot/config-{osrelease}"),
        format!("/lib/modules/{osrelease}/config"),
    ] {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            return Some(raw);
        }
    }
    None
}

fn parse_config(raw: &str) -> HashMap<String, char> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if let Some(name) = line
                .strip_prefix("# ")
                .and_then(|line| line.strip_suffix(" is not set"))
            {
                return Some((name.to_string(), 'n'));
            }
            let (name, value) = line.split_once('=')?;
            let value = value.trim();
            if value.len() == 1 && matches!(value, "y" | "m" | "n") {
                Some((name.to_string(), value.as_bytes()[0] as char))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kernel_version() {
        assert_eq!(parse_version("6.12.73+deb13-amd64"), Some((6, 12, 73)));
        assert_eq!(parse_version("6.9"), Some((6, 9, 0)));
        assert_eq!(parse_version("6.8.0-2-amd64"), Some((6, 8, 0)));
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn parses_config_values() {
        let map = parse_config(
            "CONFIG_BPF_JIT=y\nCONFIG_CGROUP_BPF=m\nCONFIG_BPF_EVENTS=n\n# CONFIG_BPF_SYSCALL is not set\n# comment\nCONFIG_BPF_JIT=z\nCONFIG_FOO=\"string\"\n",
        );
        assert_eq!(map.get("CONFIG_BPF_JIT"), Some(&'y'));
        assert_eq!(map.get("CONFIG_CGROUP_BPF"), Some(&'m'));
        assert_eq!(map.get("CONFIG_BPF_EVENTS"), Some(&'n'));
        assert_eq!(map.get("CONFIG_BPF_SYSCALL"), Some(&'n'));
        assert_eq!(map.len(), 4);
    }
}
