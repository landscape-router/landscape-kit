//! 尽力将物理以太网卡置为 UP(daemon 启动时的网卡拉起)。
//!
//! flare 的 L2 防失联通道依赖链路层收发帧:网卡处于 DOWN 状态时,即使 flare
//! 服务端在运行也无法交换 DISCOVER/AUTH 帧,恢复通道形同虚设。daemon 启动、
//! 托管 flare 服务端之前,先执行 [`bring_up_physical_interfaces`] 把发现的
//! 物理以太网卡全部置 UP。整个过程为尽力而为:枚举、读取或 `ip link set`
//! 的任何失败都只记录日志,绝不阻断 daemon 启动。

use std::path::Path;
use std::process::Command;

use super::discovery::is_physical_ethernet;

/// 尽力拉起所有处于 DOWN 状态的物理以太网卡。返回拉起成功的数量。
///
/// - 仅处理物理以太网卡(复用 [`super::discovery::is_physical_ethernet`] 判定,
///   跳过 loopback、无线与虚拟设备)且带 MAC 地址的网卡(`address` 缺失或为空的
///   直接跳过,无法作为 L2 通道介质);
/// - 已处于 UP(管理状态含 IFF_UP)的网卡跳过,不执行任何命令;
/// - 每次 `ip link set dev <name> up` 失败都打印一条诊断日志并继续。
pub(crate) fn bring_up_physical_interfaces(sys_class_net: &Path, ip_command: &Path) -> usize {
    let entries = match std::fs::read_dir(sys_class_net) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "lkit daemon: cannot enumerate network interfaces under {}: {error}",
                sys_class_net.display()
            );
            return 0;
        }
    };
    let mut brought_up = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_physical_ethernet(name, &path).unwrap_or(false) {
            continue;
        }
        if !has_mac_address(&path) {
            continue;
        }
        if is_admin_up(&path) {
            continue;
        }
        if set_interface_up(ip_command, name) {
            brought_up += 1;
        }
    }
    brought_up
}

/// 网卡是否带 MAC 地址(`/sys/class/net/<name>/address` 存在且非空)。
fn has_mac_address(path: &Path) -> bool {
    std::fs::read_to_string(path.join("address")).is_ok_and(|address| !address.trim().is_empty())
}

/// 管理状态是否已包含 IFF_UP(`/sys/class/net/<name>/flags` 的位 0)。
/// 读取或解析失败时按"未确认 UP"处理,交给 `ip link set` 去兜底。
fn is_admin_up(path: &Path) -> bool {
    const IFF_UP: u32 = 1 << 0;
    let Ok(flags) = std::fs::read_to_string(path.join("flags")) else {
        return false;
    };
    u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
        .is_ok_and(|flags| flags & IFF_UP != 0)
}

fn set_interface_up(ip_command: &Path, name: &str) -> bool {
    let output = match Command::new(ip_command)
        .args(["link", "set", "dev", name, "up"])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("lkit daemon: cannot run {}: {error}", ip_command.display());
            return false;
        }
    };
    if !output.status.success() {
        eprintln!(
            "lkit daemon: cannot bring up network interface {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let temp =
            std::env::temp_dir().join(format!("lkit-ifup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        temp
    }

    fn interface(
        dir: &std::path::Path,
        name: &str,
        type_: &str,
        flags: Option<&str>,
        mac: Option<&str>,
    ) {
        let iface = dir.join(name);
        std::fs::create_dir_all(&iface).unwrap();
        std::fs::write(iface.join("type"), type_).unwrap();
        if let Some(flags) = flags {
            std::fs::write(iface.join("flags"), flags).unwrap();
        }
        if let Some(mac) = mac {
            std::fs::write(iface.join("address"), mac).unwrap();
        }
    }

    fn fake_ip(dir: &std::path::Path, log: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("ip");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[test]
    fn brings_up_only_physical_interfaces_that_are_down() {
        let temp = test_dir("bring-up");
        let sys = temp.join("sys");
        std::fs::create_dir_all(&sys).unwrap();
        // 物理网卡,DOWN:应拉起。
        interface(&sys, "ens3", "1", Some("0x1002"), Some("02:00:00:00:00:03"));
        // 物理网卡,已 UP:不应执行命令。
        interface(&sys, "ens4", "1", Some("0x1003"), Some("02:00:00:00:00:04"));
        // 物理网卡,DOWN 但无 MAC:跳过。
        interface(&sys, "ens5", "1", Some("0x1002"), None);
        // 物理网卡,DOWN 但 MAC 为空:跳过。
        interface(&sys, "ens6", "1", Some("0x1002"), Some(""));
        // loopback:按名字跳过。
        interface(&sys, "lo", "772", Some("0x1003"), Some("00:00:00:00:00:00"));
        // 无线网卡:跳过。
        interface(&sys, "wlan0", "1", None, Some("02:00:00:00:00:05"));
        std::fs::write(sys.join("wlan0/wireless"), "").unwrap();
        // 虚拟网卡:规范路径含 /devices/virtual/net/,跳过。
        let virtual_type = temp.join("devices/virtual/net/veth0");
        std::fs::create_dir_all(&virtual_type).unwrap();
        std::fs::write(virtual_type.join("type"), "1").unwrap();
        std::fs::write(virtual_type.join("flags"), "0x1002").unwrap();
        std::fs::write(virtual_type.join("address"), "02:00:00:00:00:06").unwrap();
        std::os::unix::fs::symlink("../devices/virtual/net/veth0", sys.join("veth0")).unwrap();

        let log = temp.join("ip.log");
        let ip = fake_ip(&temp, &log);

        let brought_up = bring_up_physical_interfaces(&sys, &ip);
        assert_eq!(brought_up, 1);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "link set dev ens3 up\n"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn missing_flags_file_still_attempts_bring_up() {
        let temp = test_dir("missing-flags");
        let sys = temp.join("sys");
        std::fs::create_dir_all(&sys).unwrap();
        interface(&sys, "ens3", "1", None, Some("02:00:00:00:00:03"));

        let log = temp.join("ip.log");
        let ip = fake_ip(&temp, &log);

        let brought_up = bring_up_physical_interfaces(&sys, &ip);
        assert_eq!(brought_up, 1);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "link set dev ens3 up\n"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn failed_ip_link_set_is_not_fatal() {
        let temp = test_dir("ip-failure");
        let sys = temp.join("sys");
        std::fs::create_dir_all(&sys).unwrap();
        interface(&sys, "ens3", "1", Some("0x1002"), Some("02:00:00:00:00:03"));

        let failing = temp.join("ip");
        std::fs::write(&failing, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();

        let brought_up = bring_up_physical_interfaces(&sys, &failing);
        assert_eq!(brought_up, 0);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
