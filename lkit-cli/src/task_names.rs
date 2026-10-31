//! All application task and OS-thread names for this crate.
//! Keep OS-thread names below Linux's 16-byte limit (including the NUL).

pub const RUNTIME: &str = "lkit-rt";
pub const BACKUP_RUNTIME: &str = "lkit-bak-rt";
pub const UPDATE_RUNTIME: &str = "lkit-upd-rt";
pub const DAEMON_FLARE: &str = "daemon.flare";
pub const FLARE_SHUTDOWN: &str = "flare.server.shutdown";
pub const FLARE_CONNECTION: &str = "flare.server.connection";
pub const CONSOLE_DAEMON: &str = "lkit-daemon";
pub const CONSOLE_PREFLIGHT: &str = "lkit-preflight";
pub const CONSOLE_SOFTWARE_INSTALL: &str = "lkit-sw-install";
pub const CONSOLE_BASE_INSTALL: &str = "lkit-base-inst";
pub const CONSOLE_BACKUP_LIST: &str = "lkit-bak-list";
pub const CONSOLE_BACKUP_CREATE: &str = "lkit-bak-create";
pub const CONSOLE_BACKUP_VERIFY: &str = "lkit-bak-verify";
pub const CONSOLE_UPDATE: &str = "lkit-update";
pub const CONSOLE_MIRROR_PROBE: &str = "lkit-mir-probe";
pub const CONSOLE_MIRROR_APPLY: &str = "lkit-mir-apply";
pub const MIRROR_PROBE: &str = "lkit-probe";
