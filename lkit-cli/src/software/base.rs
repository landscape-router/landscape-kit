use std::fs::Metadata;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::ValueEnum;

use super::SoftwareError;

/// Landscape 运行依赖的基础系统包。新增依赖时在此添加变体并补充二进制探测
/// 与各包管理器的包名映射。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, ValueEnum)]
pub(crate) enum BasePackage {
    /// pppd 拨号客户端
    #[value(name = "ppp")]
    Ppp,
    /// iproute2 的 ip 命令
    #[value(name = "iproute2")]
    Iproute2,
    /// iw 无线配置工具
    #[value(name = "iw")]
    Iw,
    /// hostapd 无线热点
    #[value(name = "hostapd")]
    Hostapd,
    /// procps 的 sysctl
    #[value(name = "procps")]
    Procps,
}

impl BasePackage {
    pub(crate) fn all() -> [Self; 5] {
        [
            Self::Ppp,
            Self::Iproute2,
            Self::Iw,
            Self::Hostapd,
            Self::Procps,
        ]
    }

    /// 面板与弹框中展示的名称,格式为「二进制(包名)」。
    pub(crate) fn label(self) -> String {
        match self {
            Self::Ppp => crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGE_PPP),
            Self::Iproute2 => crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGE_IPROUTE2),
            Self::Iw => crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGE_IW),
            Self::Hostapd => crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGE_HOSTAPD),
            Self::Procps => crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGE_PROCPS),
        }
    }

    /// CLI 参数与包标识,与 `ValueEnum` 的 value name 一致。
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Ppp => "ppp",
            Self::Iproute2 => "iproute2",
            Self::Iw => "iw",
            Self::Hostapd => "hostapd",
            Self::Procps => "procps",
        }
    }

    /// 探测安装状态的二进制名。
    fn binary(self) -> &'static str {
        match self {
            Self::Ppp => "pppd",
            Self::Iproute2 => "ip",
            Self::Iw => "iw",
            Self::Hostapd => "hostapd",
            Self::Procps => "sysctl",
        }
    }

    /// 当前主机上是否已安装(按 PATH 探测二进制)。
    pub(crate) fn installed(self) -> bool {
        find_in_path(self.binary()).is_some()
    }

    /// 该包在指定包管理器下的包名。
    pub(crate) fn package_name(self, manager: PackageManager) -> &'static str {
        match (self, manager) {
            (Self::Ppp, _) => "ppp",
            (Self::Iproute2, PackageManager::Dnf) | (Self::Iproute2, PackageManager::Yum) => {
                "iproute"
            }
            (Self::Iproute2, _) => "iproute2",
            (Self::Iw, _) => "iw",
            (Self::Hostapd, _) => "hostapd",
            (Self::Procps, PackageManager::Dnf)
            | (Self::Procps, PackageManager::Yum)
            | (Self::Procps, PackageManager::Pacman) => "procps-ng",
            (Self::Procps, _) => "procps",
        }
    }
}

/// 支持的包管理器。`detect` 按 PATH 探测命令,生产主机通常只有一个。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
}

impl PackageManager {
    fn detect() -> Option<Self> {
        [
            ("apt-get", Self::Apt),
            ("dnf", Self::Dnf),
            ("yum", Self::Yum),
            ("pacman", Self::Pacman),
            ("zypper", Self::Zypper),
        ]
        .into_iter()
        .find_map(|(command, manager)| find_in_path(command).map(|_| manager))
    }

    fn command(self) -> &'static str {
        match self {
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Yum => "yum",
            Self::Pacman => "pacman",
            Self::Zypper => "zypper",
        }
    }

    /// 安装命令的前置参数(不含包名)。
    fn install_prefix(self) -> &'static [&'static str] {
        match self {
            Self::Apt | Self::Dnf | Self::Yum | Self::Zypper => &["install", "-y"],
            Self::Pacman => &["-S", "--noconfirm"],
        }
    }
}

/// 基础包弹框中的单包条目:已安装的包置灰并默认选中(不可取消),
/// 缺失的包默认勾选,可切换。
#[derive(Clone, Debug)]
pub(crate) struct BasePackageEntry {
    pub(crate) package: BasePackage,
    pub(crate) installed: bool,
    pub(crate) selected: bool,
}

/// 基础包多选弹框状态。末行是「确认安装」动作行。
#[derive(Clone, Debug)]
pub(crate) struct BasePackageDialog {
    pub(crate) entries: Vec<BasePackageEntry>,
    pub(crate) cursor: usize,
}

impl BasePackageDialog {
    pub(crate) fn open() -> Self {
        Self {
            entries: BasePackage::all()
                .into_iter()
                .map(|package| BasePackageEntry {
                    package,
                    installed: package.installed(),
                    selected: !package.installed(),
                })
                .collect(),
            cursor: 0,
        }
    }

    /// 弹框行数:包行 + 确认动作行。
    pub(crate) fn row_count(&self) -> usize {
        self.entries.len() + 1
    }

    pub(crate) fn move_cursor(&mut self, forward: bool) {
        if forward {
            self.cursor = (self.cursor + 1).min(self.row_count() - 1);
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    /// 光标是否落在确认动作行。
    pub(crate) fn on_confirm_row(&self) -> bool {
        self.cursor == self.entries.len()
    }

    /// 切换当前包行勾选;已安装的包不可切换。
    pub(crate) fn toggle(&mut self) -> Result<(), String> {
        let Some(entry) = self.entries.get_mut(self.cursor) else {
            return Ok(());
        };
        if entry.installed {
            return Err(crate::tr!(
                crate::keys::CONSOLE_BASE_PACKAGES_ALREADY_INSTALLED,
                package = entry.package.label()
            ));
        }
        entry.selected = !entry.selected;
        Ok(())
    }

    /// 当前勾选且缺失、需要安装的包。
    pub(crate) fn selected_packages(&self) -> Vec<BasePackage> {
        self.entries
            .iter()
            .filter(|entry| entry.selected && !entry.installed)
            .map(|entry| entry.package)
            .collect()
    }
}

/// 在当前 PATH 中探测可执行文件,返回完整路径。
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        std::fs::metadata(&candidate)
            .ok()
            .filter(is_executable)
            .map(|_| candidate)
    })
}

fn is_executable(metadata: &Metadata) -> bool {
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

/// 安装指定的基础包,跳过当前主机上已经安装的包。包管理器按 PATH 探测;
/// 无法识别时返回错误。`stream` 为 true 时包管理器输出直接流到终端
/// (CLI 模式);false 时捕获输出,仅失败时透出错误信息(TUI worker 模式)。
/// `cancel` 置位后正在运行的包管理器命令会被终止,安装返回取消错误。
pub(crate) fn install(
    packages: &[BasePackage],
    stream: bool,
    cancel: &AtomicBool,
) -> Result<(), SoftwareError> {
    let missing: Vec<BasePackage> = packages
        .iter()
        .copied()
        .filter(|package| !package.installed())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let manager = PackageManager::detect().ok_or_else(|| {
        SoftwareError::Message(crate::tr!(crate::keys::SOFTWARE_BASE_PACKAGES_NO_MANAGER))
    })?;
    // apt 基础镜像可能没有包列表:先刷新一次,否则 `apt-get install` 会报
    // "Unable to locate package"。dnf/yum/zypper 安装时会自动刷新元数据。
    if manager == PackageManager::Apt {
        run_command("apt-get", &["update", "-y"], stream, cancel)?;
    }
    let mut args: Vec<&str> = manager.install_prefix().to_vec();
    args.extend(missing.iter().map(|package| package.package_name(manager)));
    run_command(manager.command(), &args, stream, cancel)
}

/// 运行包管理器命令。`stream` 为 true 时继承 stdio 并透出原始输出;
/// 否则捕获输出,仅在失败时把 stderr 并入错误信息。
/// `cancel` 置位时正在运行的命令会被 SIGTERM 终止并返回取消错误;
/// 子进程设置 PDEATHSIG,父进程(lkit)退出时自动终止,避免 Ctrl+C 后残留。
fn run_command(
    program: &str,
    args: &[&str],
    stream: bool,
    cancel: &AtomicBool,
) -> Result<(), SoftwareError> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec 在 fork 后 exec 前运行;仅设置 PDEATHSIG,
        // 不触碰进程状态,不调用异步不安全函数。
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    if stream {
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(SoftwareError::Message(format!(
                "{program} exited with status {status}"
            )))
        }
    } else {
        run_captured(command, program, cancel)
    }
}

/// 捕获输出运行命令,轮询取消标志;取消时终止子进程并返回取消错误。
fn run_captured(
    mut command: std::process::Command,
    program: &str,
    cancel: &AtomicBool,
) -> Result<(), SoftwareError> {
    use std::os::unix::process::ExitStatusExt;
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SoftwareError::Message(crate::tr!(
                crate::keys::SOFTWARE_CANCELLED
            )));
        }
        match child.try_wait()? {
            Some(status) => {
                let mut out = Vec::new();
                let mut err = Vec::new();
                if let Some(pipe) = stdout.as_mut() {
                    let _ = pipe.read_to_end(&mut out);
                }
                if let Some(pipe) = stderr.as_mut() {
                    let _ = pipe.read_to_end(&mut err);
                }
                if status.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&err);
                let stderr = stderr.trim();
                let _ = out;
                return Err(SoftwareError::Message(if stderr.is_empty() {
                    format!(
                        "{program} exited with status {}",
                        status.signal().unwrap_or(0)
                    )
                } else {
                    format!("{program}: {stderr}")
                }));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_path(path: &std::path::Path, name: &str) -> Option<PathBuf> {
        let candidate = path.join(name);
        std::fs::metadata(&candidate)
            .ok()
            .filter(is_executable)
            .map(|_| candidate)
    }

    #[test]
    fn maps_package_names_per_manager() {
        let cases = [
            (BasePackage::Ppp, "ppp"),
            (BasePackage::Iw, "iw"),
            (BasePackage::Hostapd, "hostapd"),
        ];
        for (package, expected) in cases {
            for manager in [
                PackageManager::Apt,
                PackageManager::Dnf,
                PackageManager::Yum,
                PackageManager::Pacman,
                PackageManager::Zypper,
            ] {
                assert_eq!(package.package_name(manager), expected);
            }
        }
        assert_eq!(
            BasePackage::Iproute2.package_name(PackageManager::Apt),
            "iproute2"
        );
        assert_eq!(
            BasePackage::Iproute2.package_name(PackageManager::Dnf),
            "iproute"
        );
        assert_eq!(
            BasePackage::Iproute2.package_name(PackageManager::Yum),
            "iproute"
        );
        assert_eq!(
            BasePackage::Iproute2.package_name(PackageManager::Pacman),
            "iproute2"
        );
        assert_eq!(
            BasePackage::Procps.package_name(PackageManager::Apt),
            "procps"
        );
        assert_eq!(
            BasePackage::Procps.package_name(PackageManager::Dnf),
            "procps-ng"
        );
        assert_eq!(
            BasePackage::Procps.package_name(PackageManager::Pacman),
            "procps-ng"
        );
        assert_eq!(
            BasePackage::Procps.package_name(PackageManager::Zypper),
            "procps"
        );
    }

    #[test]
    fn install_prefixes_match_expected_commands() {
        assert_eq!(PackageManager::Apt.install_prefix(), &["install", "-y"]);
        assert_eq!(
            PackageManager::Pacman.install_prefix(),
            &["-S", "--noconfirm"]
        );
    }

    #[test]
    fn probes_binaries_in_path_directories() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-base-package-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("pppd");
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        assert!(probe_path(&dir, "pppd").is_some());
        assert!(probe_path(&dir, "ip").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
