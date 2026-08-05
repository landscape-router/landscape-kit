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
    let mut result = CheckResult::new("kernel.version", crate::tr!("Kernel version", "内核版本"));
    match std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        Ok(raw) => {
            let version = raw.trim().to_string();
            result.value = version.clone();
            match parse_version(&version) {
                Some((major, minor, _)) if (major, minor) >= (6, 9) => {
                    result.set(Status::Pass, version, crate::tr!("The kernel meets the 6.9+ requirement", "内核版本满足 6.9+ 要求"))
                }
                Some((_, _, _)) => result
                    .set(
                        Status::Error,
                        version.clone(),
                        crate::trf!(("Kernel version is below the requirement: >= 6.9 required, current {version}"), ("内核版本低于要求：需要 >= 6.9，当前为 {version}")),
                    )
                    .suggestion(crate::tr!("Upgrade the kernel to version 6.9 or later and retry", "请升级内核到 6.9 或更高版本后重试")),
                None => result.set(Status::Unknown, version, crate::tr!("Unable to parse the kernel version", "无法解析内核版本号")),
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            crate::tr!("unavailable", "无法读取"),
            crate::trf!(
                ("Unable to read /proc/sys/kernel/osrelease: {err}"),
                ("无法读取 /proc/sys/kernel/osrelease：{err}")
            ),
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
        crate::tr!("BPF subsystem and BPF JIT", "BPF 子系统与 BPF JIT"),
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
                "BPF_PROG_GET_NEXT_ID probe succeeded",
                "BPF_PROG_GET_NEXT_ID 探测成功"
            ));
        } else {
            let errno = std::io::Error::last_os_error();
            let is_root = unsafe { libc::geteuid() == 0 };
            match errno.raw_os_error() {
                Some(libc::ENOENT) => {
                    result = result.detail(crate::tr!(
                        "BPF_PROG_GET_NEXT_ID returned ENOENT; the BPF subsystem is available",
                        "BPF_PROG_GET_NEXT_ID 返回 ENOENT，BPF 子系统可用"
                    ));
                }
                Some(libc::ENOSYS) => {
                    return result
                        .set(
                            Status::Error,
                            crate::tr!("BPF syscall unavailable", "BPF syscall 不可用"),
                            crate::tr!(
                                "The kernel does not support the BPF syscall (ENOSYS)",
                                "内核不支持 BPF syscall（ENOSYS）"
                            ),
                        )
                        .suggestion(crate::tr!(
                            "Use a kernel that supports eBPF",
                            "当前内核不支持 eBPF，请更换支持 eBPF 的内核"
                        ));
                }
                Some(libc::EPERM) | Some(libc::EACCES) if is_root => {
                    return result
                        .set(
                            Status::Error,
                            crate::tr!("BPF denied", "BPF 被拒绝"),
                            crate::tr!("The BPF syscall returned a permission error as root and may be blocked by seccomp or an LSM", "root 身份下 BPF syscall 仍返回权限错误，可能被 seccomp 或 LSM 拦截"),
                        )
                        .suggestion(crate::tr!("Check whether the process is restricted by seccomp or an LSM", "检查进程是否被 seccomp 或 LSM 限制"));
                }
                Some(libc::EPERM) | Some(libc::EACCES) => {
                    return result
                        .set(
                            Status::Unknown,
                            crate::tr!("permission denied", "权限不足"),
                            crate::tr!(
                                "The current identity cannot probe the BPF subsystem",
                                "当前身份无权探测 BPF 子系统"
                            ),
                        )
                        .suggestion(crate::tr!(
                            "Run lkit check as root",
                            "请以 root 身份运行 lkit check"
                        ));
                }
                Some(other) => {
                    return result.set(
                        Status::Unknown,
                        format!("errno {other}"),
                        crate::trf!(
                            ("BPF probe returned an unexpected error: {errno}"),
                            ("BPF 探测返回未预期错误：{errno}")
                        ),
                    );
                }
                None => {
                    return result.set(
                        Status::Unknown,
                        crate::tr!("unknown error", "未知错误"),
                        crate::trf!(("BPF probe failed: {errno}"), ("BPF 探测失败：{errno}")),
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
                        crate::trf!(
                            ("BPF subsystem available, JIT enabled ({value})"),
                            ("BPF 子系统可用，JIT 已启用（{value}）")
                        ),
                        crate::tr!(
                            "Both the BPF syscall and JIT are available",
                            "BPF syscall 与 JIT 均可用"
                        ),
                    ),
                    "0" => result
                        .set(
                            Status::Error,
                            crate::trf!(("JIT disabled ({value})"), ("JIT 已禁用（{value}）")),
                            crate::tr!("BPF JIT is disabled", "BPF JIT 处于禁用状态"),
                        )
                        .suggestion(crate::tr!(
                            "Enable JIT: sysctl -w net.core.bpf_jit_enable=1",
                            "启用 JIT：sysctl -w net.core.bpf_jit_enable=1"
                        )),
                    _ => result.set(
                        Status::Unknown,
                        value,
                        crate::tr!("Unrecognized BPF JIT status", "无法识别 BPF JIT 状态"),
                    ),
                }
            }
            Err(err) => match config_value("CONFIG_BPF_JIT") {
                Some('y') => result.set(
                    Status::Pass,
                    crate::tr!(
                        "JIT status file unreadable; built into kernel",
                        "JIT 状态文件不可读，配置为内置"
                    ),
                    crate::trf!(
                        ("{err}; kernel configuration has CONFIG_BPF_JIT=y"),
                        ("{err}；内核配置 CONFIG_BPF_JIT=y")
                    ),
                ),
                Some('m') => result.set(
                    Status::Unknown,
                    crate::tr!("JIT is a module", "JIT 为模块"),
                    crate::tr!(
                        "Unable to confirm whether the BPF JIT module is loaded",
                        "无法确认 BPF JIT 模块是否已加载"
                    ),
                ),
                Some('n') => result
                    .set(
                        Status::Error,
                        "CONFIG_BPF_JIT=n",
                        crate::tr!(
                            "BPF JIT was disabled when the kernel was built",
                            "内核编译时未启用 BPF JIT"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Use a kernel built with CONFIG_BPF_JIT",
                        "需使用启用 CONFIG_BPF_JIT 的内核"
                    )),
                _ => result.set(
                    Status::Unknown,
                    crate::tr!("unknown", "无法确认"),
                    crate::trf!(
                        ("Unable to read bpf_jit_enable ({err}) or the kernel configuration"),
                        ("无法读取 bpf_jit_enable（{err}），也无法读取内核配置")
                    ),
                ),
            },
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        result.set(
            Status::Unknown,
            crate::tr!("not Linux", "非 Linux"),
            crate::tr!(
                "The current platform cannot probe BPF",
                "当前平台无法探测 BPF"
            ),
        )
    }
}

fn btf() -> CheckResult {
    let result = CheckResult::new(
        "kernel.btf",
        crate::tr!("Kernel BTF information", "内核 BTF 信息"),
    );
    let path = "/sys/kernel/btf/vmlinux";
    match std::fs::File::open(path) {
        Ok(_) => result.set(
            Status::Pass,
            crate::tr!("present and readable", "存在且可读取"),
            crate::tr!("Kernel BTF information is available", "内核 BTF 信息可用"),
        ),
        Err(err) if Path::new(path).exists() => result.set(
            Status::Unknown,
            crate::tr!("present but unreadable", "存在但不可读取"),
            crate::trf!(
                ("{path} exists but cannot be read: {err}"),
                ("{path} 存在但无法读取：{err}")
            ),
        ),
        Err(_) => result
            .set(
                Status::Error,
                crate::tr!("missing", "不存在"),
                crate::trf!(
                    ("{path} does not exist; the kernel does not provide BTF information"),
                    ("{path} 不存在，内核未提供 BTF 信息")
                ),
            )
            .suggestion(crate::tr!(
                "Use a kernel with BTF support, such as one built with CONFIG_DEBUG_INFO_BTF",
                "需要支持 BTF 的内核（如启用 CONFIG_DEBUG_INFO_BTF 的内核）"
            )),
    }
}

fn cgroup() -> CheckResult {
    let result = CheckResult::new(
        "kernel.cgroup",
        crate::tr!("Cgroup filesystem", "Cgroup 文件系统"),
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
                            crate::trf!(("mounted ({fstype})"), ("已挂载（{fstype}）")),
                            crate::tr!("The Cgroup filesystem is available", "Cgroup 文件系统可用"),
                        )
                    } else {
                        result.set(
                            Status::Unknown,
                            crate::trf!(
                                ("mounted ({fstype}) but unreadable"),
                                ("已挂载（{fstype}）但不可读取")
                            ),
                            crate::tr!("/sys/fs/cgroup is unreadable", "/sys/fs/cgroup 不可读"),
                        )
                    }
                }
                None => result
                    .set(
                        Status::Error,
                        crate::tr!("not mounted", "未挂载"),
                        crate::tr!(
                            "No Cgroup filesystem is mounted at /sys/fs/cgroup",
                            "/sys/fs/cgroup 未挂载 Cgroup 文件系统"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Check whether the kernel has Cgroup support enabled",
                        "检查内核是否启用 Cgroup 支持"
                    )),
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            crate::tr!("unavailable", "无法读取"),
            crate::trf!(
                ("Unable to read /proc/self/mounts: {err}"),
                ("无法读取 /proc/self/mounts：{err}")
            ),
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
                    crate::tr!("available", "可用"),
                    crate::tr!(
                        "The cgroup v2 CPU controller is enabled",
                        "cgroup v2 CPU controller 已启用"
                    ),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        crate::tr!("disabled", "未启用"),
                        crate::tr!(
                            "The cgroup v2 CPU controller is not in the available list",
                            "cgroup v2 CPU controller 未在可用列表中"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Add cpu to cgroup.controllers / subtree_control",
                        "将 cpu 加入 cgroup.controllers / subtree_control"
                    ))
            }
        }
        Err(_) => {
            if Path::new("/sys/fs/cgroup/cpu").is_dir() {
                result.set(
                    Status::Pass,
                    crate::tr!("available", "可用"),
                    crate::tr!(
                        "The cgroup v1 cpu controller is mounted",
                        "cgroup v1 cpu controller 已挂载"
                    ),
                )
            } else if Path::new("/sys/fs/cgroup").exists() {
                result.set(
                    Status::Unknown,
                    crate::tr!("unknown", "无法确认"),
                    crate::tr!(
                        "Unable to read cgroup controller information",
                        "无法读取 cgroup 控制器信息"
                    ),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        crate::tr!("not mounted", "未挂载"),
                        crate::tr!(
                            "The Cgroup filesystem is unavailable",
                            "Cgroup 文件系统不可用"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Check whether the kernel has Cgroup support enabled",
                        "检查内核是否启用 Cgroup 支持"
                    ))
            }
        }
    }
}

fn cgroup_bpf() -> CheckResult {
    config_check(
        "kernel.cgroup_bpf",
        crate::tr!("Cgroup BPF support", "Cgroup BPF 支持"),
        "CONFIG_CGROUP_BPF",
    )
}

fn bpf_events() -> CheckResult {
    config_check(
        "kernel.bpf_events",
        crate::tr!("BPF events support", "BPF events 支持"),
        "CONFIG_BPF_EVENTS",
    )
}

fn config_check(id: &'static str, title: &'static str, name: &str) -> CheckResult {
    let result = CheckResult::new(id, title);
    match config_value(name) {
        Some('y') => result.set(
            Status::Pass,
            crate::tr!("enabled", "已启用"),
            crate::trf!(("{name}=y (built-in)"), ("{name}=y（内置）")),
        ),
        Some('m') => result.set(
            Status::Unknown,
            crate::tr!("module", "模块形式"),
            crate::trf!(
                ("{name}=m; unable to confirm whether it is loaded"),
                ("{name}=m，无法确认是否已实际加载")
            ),
        ),
        Some('n') => result
            .set(
                Status::Error,
                crate::tr!("disabled", "未启用"),
                format!("{name}=n"),
            )
            .suggestion(crate::tr!(
                "Use a kernel with this configuration option enabled",
                "需使用启用该配置项的内核"
            )),
        _ => result.set(
            Status::Unknown,
            crate::tr!("unknown", "无法确认"),
            crate::tr!(
                "Unable to read the kernel configuration, so this capability cannot be confirmed",
                "无法读取内核配置，不能判定该能力已启用"
            ),
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
