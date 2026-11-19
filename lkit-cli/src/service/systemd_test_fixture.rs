use super::Systemd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A systemctl double for workflow tests. All state lives under the test directory.
pub(crate) fn new(dir: &Path) -> Systemd {
    let systemd = Systemd {
        systemctl: dir.join("systemctl"),
        system_unit_dir: dir.join("units"),
        run_systemd_dir: dir.join("run"),
        pid1_is_systemd: true,
        resolv_conf: dir.join("resolv.conf"),
    };
    std::fs::create_dir_all(&systemd.system_unit_dir).unwrap();
    std::fs::create_dir_all(&systemd.run_systemd_dir).unwrap();
    std::fs::write(dir.join("state"), "inactive\n").unwrap();
    std::fs::write(
        &systemd.systemctl,
        format!(r#"#!/bin/sh
set -eu
cd "$(dirname "$0")"
unit=landscape-router.service
link="units/multi-user.target.wants/$unit"
case "$*" in
  "show --property=Version") echo Version=252.fake ;;
  "enable $unit") mkdir -p units/multi-user.target.wants && ln -sf "$(readlink "units/$unit")" "$link" ;;
  "disable $unit") rm -f "$link" ;;
  "is-enabled $unit") if test -L "$link"; then echo enabled; else echo disabled; exit 1; fi ;;
  "start $unit"|"restart $unit") echo active >state ;;
  "stop $unit") echo inactive >state ;;
  "show --property=MainPID --value $unit") if test "$(cat state)" = active; then echo {}; else echo 0; fi ;;
  "show --property=ActiveState --value $unit"|"is-active $unit") cat state ;;
  daemon-reload) ;;
  *) exit 1 ;;
esac
"#, std::process::id())
    ).unwrap();
    std::fs::set_permissions(&systemd.systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
    systemd
}
