//! Explicit OpenRC cgroup preparation. No daemon-start side effects, no raw
//! mounts, hierarchy migration, controller writes, or automatic rollback.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

const ROOT: &str = "/sys/fs/cgroup";
const SERVICE: &str = "/etc/init.d/cgroups";
const BOOT_LINK: &str = "/etc/runlevels/boot/cgroups";

pub(crate) fn is_openrc() -> bool {
    crate::service::manager::host_kind()
        == Some(crate::service::manager::ServiceManagerKind::Openrc)
}

pub(crate) fn available() -> bool {
    is_openrc()
        && [SERVICE, "/sbin/rc-service", "/sbin/rc-update"]
            .iter()
            .all(|path| executable(Path::new(path)))
}

pub(crate) fn boot_enabled() -> bool {
    boot_link_matches(Path::new(SERVICE), Path::new(BOOT_LINK))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn boot_link_matches(service: &Path, link: &Path) -> bool {
    if !executable(service) {
        return false;
    }
    match (fs::canonicalize(service), fs::canonicalize(link)) {
        (Ok(service), Ok(link)) => service == link,
        _ => false,
    }
}

pub(crate) fn enable_openrc() -> Result<(), String> {
    enable_with(&mut Host)
}

trait Operations {
    fn root(&self) -> bool;
    fn available(&self) -> bool;
    fn mounts(&self) -> Result<String, String>;
    fn cpu_available(&self) -> bool;
    fn boot_enabled(&self) -> bool;
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String>;
}

#[derive(Debug, PartialEq, Eq)]
enum Layout {
    Missing,
    Unified,
    Conflict,
}

fn layout(mounts: &str) -> Layout {
    let mut found = false;
    for line in mounts.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let mountpoint = fields[1];
        let filesystem = fields[2];
        if filesystem == "cgroup" || filesystem == "cgroup2" || mountpoint == ROOT {
            if mountpoint != ROOT
                || filesystem != "cgroup2"
                || !fields[3].split(',').any(|option| option == "rw")
                || found
            {
                return Layout::Conflict;
            }
            found = true;
        }
    }
    if found {
        Layout::Unified
    } else {
        Layout::Missing
    }
}

fn enable_with(ops: &mut impl Operations) -> Result<(), String> {
    if !ops.root() {
        return Err("root privileges are required to enable cgroups".into());
    }
    if !ops.available() {
        return Err(
            "OpenRC cgroups service is unavailable; configure cgroups using the host init system"
                .into(),
        );
    }
    match layout(&ops.mounts()?) {
        Layout::Conflict => return Err("existing cgroup v1, hybrid, read-only, or conflicting mounts are preserved; manual configuration is required".into()),
        Layout::Missing => ops.run("/sbin/rc-service", &["cgroups", "start"] )?,
        Layout::Unified => {}
    }
    if layout(&ops.mounts()?) != Layout::Unified || !ops.cpu_available() {
        return Err("OpenRC did not provide a writable cgroup v2 hierarchy with a CPU controller; no hierarchy conversion was attempted".into());
    }
    if !ops.boot_enabled() {
        ops.run("/sbin/rc-update", &["add", "cgroups", "boot"])
            .map_err(|error| {
                format!("cgroups is ready now, but enabling it at boot failed: {error}")
            })?;
    }
    if !ops.boot_enabled() {
        return Err(
            "cgroups is ready now, but its boot runlevel registration could not be verified".into(),
        );
    }
    Ok(())
}

struct Host;
impl Operations for Host {
    fn root(&self) -> bool {
        fs::read_to_string("/proc/self/status").is_ok_and(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|uids| uids.split_whitespace().nth(1))
                == Some("0")
        })
    }
    fn available(&self) -> bool {
        available()
    }
    fn mounts(&self) -> Result<String, String> {
        fs::read_to_string("/proc/self/mounts").map_err(|error| error.to_string())
    }
    fn cpu_available(&self) -> bool {
        fs::read_to_string(format!("{ROOT}/cgroup.controllers")).is_ok_and(|controllers| {
            controllers
                .split_whitespace()
                .any(|controller| controller == "cpu")
        })
    }
    fn boot_enabled(&self) -> bool {
        boot_enabled()
    }
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("{program}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{program} {}: {} ({})",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim(),
                output.status
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const UNIFIED: &str = "cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec 0 0\n";
    struct Fake {
        root: bool,
        available: bool,
        mounts: String,
        cpu: bool,
        boot: bool,
        fail_at: Option<usize>,
        calls: Vec<String>,
        start_mounts: bool,
        register_boot: bool,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                root: true,
                available: true,
                mounts: String::new(),
                cpu: true,
                boot: false,
                fail_at: None,
                calls: vec![],
                start_mounts: true,
                register_boot: true,
            }
        }
    }
    impl Operations for Fake {
        fn root(&self) -> bool {
            self.root
        }
        fn available(&self) -> bool {
            self.available
        }
        fn mounts(&self) -> Result<String, String> {
            Ok(self.mounts.clone())
        }
        fn cpu_available(&self) -> bool {
            self.cpu
        }
        fn boot_enabled(&self) -> bool {
            self.boot
        }
        fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
            self.calls.push(format!("{program} {}", args.join(" ")));
            if self.fail_at == Some(self.calls.len()) {
                return Err("fixture failure".into());
            }
            if program.ends_with("rc-service") && self.start_mounts {
                self.mounts = UNIFIED.into();
            }
            if program.ends_with("rc-update") && self.register_boot {
                self.boot = true;
            }
            Ok(())
        }
    }
    #[test]
    fn starts_then_verifies_and_always_registers_boot() {
        let mut ops = Fake::default();
        enable_with(&mut ops).unwrap();
        assert_eq!(
            ops.calls,
            [
                "/sbin/rc-service cgroups start",
                "/sbin/rc-update add cgroups boot"
            ]
        );
        enable_with(&mut ops).unwrap();
        assert_eq!(
            ops.calls.len(),
            2,
            "already ready and persistent is a no-op"
        );
    }
    #[test]
    fn existing_mount_still_requires_boot_registration() {
        let mut ops = Fake {
            mounts: UNIFIED.into(),
            ..Fake::default()
        };
        enable_with(&mut ops).unwrap();
        assert_eq!(ops.calls, ["/sbin/rc-update add cgroups boot"]);
    }
    #[test]
    fn rejects_non_root_and_unavailable_hosts_without_commands() {
        for mut ops in [
            Fake {
                root: false,
                ..Fake::default()
            },
            Fake {
                available: false,
                ..Fake::default()
            },
        ] {
            assert!(enable_with(&mut ops).is_err());
            assert!(ops.calls.is_empty());
        }
    }
    #[test]
    fn preserves_existing_v1_hybrid_readonly_and_conflicting_mounts() {
        for mounts in [
            "cgroup /sys/fs/cgroup/cpu cgroup rw,cpu 0 0\n",
            "cgroup2 /sys/fs/cgroup/unified cgroup2 rw 0 0\n",
            "cgroup2 /sys/fs/cgroup cgroup2 ro 0 0\n",
            "tmpfs /sys/fs/cgroup tmpfs rw 0 0\n",
        ] {
            let mut ops = Fake {
                mounts: mounts.into(),
                ..Fake::default()
            };
            assert!(enable_with(&mut ops).is_err());
            assert!(ops.calls.is_empty());
        }
    }
    #[test]
    fn failed_commands_or_failed_verification_never_report_success() {
        for mut ops in [
            Fake {
                fail_at: Some(1),
                ..Fake::default()
            },
            Fake {
                fail_at: Some(2),
                ..Fake::default()
            },
            Fake {
                start_mounts: false,
                ..Fake::default()
            },
            Fake {
                cpu: false,
                ..Fake::default()
            },
            Fake {
                register_boot: false,
                ..Fake::default()
            },
        ] {
            assert!(enable_with(&mut ops).is_err());
            assert!(
                ops.calls
                    .iter()
                    .all(|call| !call.contains("stop") && !call.contains("mount -"))
            );
        }
    }
    #[test]
    fn verifies_boot_link_target_in_a_temporary_directory() {
        let root = std::env::temp_dir().join(format!("lkit-cgroup-link-{}", uuid_for_test()));
        fs::create_dir_all(&root).unwrap();
        let service = root.join("cgroups");
        let link = root.join("boot-cgroups");
        fs::write(&service, "fixture").unwrap();
        fs::set_permissions(&service, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!boot_link_matches(&service, &link));
        std::os::unix::fs::symlink(&service, &link).unwrap();
        assert!(boot_link_matches(&service, &link));
        fs::set_permissions(&service, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            !boot_link_matches(&service, &link),
            "a non-executable service cannot prepare Cgroup at boot"
        );
        fs::remove_dir_all(root).unwrap();
    }
    fn uuid_for_test() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
