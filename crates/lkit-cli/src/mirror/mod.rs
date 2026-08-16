pub(crate) mod apt;
pub(crate) mod backend;
pub(crate) mod common;
pub(crate) mod detect;
pub(crate) mod dnf;
pub(crate) mod pacman;

use std::path::PathBuf;
use std::sync::OnceLock;

use clap::ValueEnum;
use thiserror::Error;

/// 生产环境的系统文件根路径。
pub(crate) const BACKUP_ROOT: &str = "/var/lib/lkit/mirror-backup";
pub(crate) const OS_RELEASE_PATH: &str = "/etc/os-release";
pub(crate) const APT_SOURCES_LIST: &str = "/etc/apt/sources.list";
pub(crate) const APT_SOURCES_LIST_D: &str = "/etc/apt/sources.list.d";
pub(crate) const DNF_REPOS_DIR: &str = "/etc/yum.repos.d";
pub(crate) const PACMAN_MIRRORLIST: &str = "/etc/pacman.d/mirrorlist";

/// mirror 模块读写的主机路径与权限策略。生产环境恒为 [`MirrorPaths::production`]；
/// `test-support` 构建下测试可以覆盖为临时目录，隔离真实系统文件。
#[derive(Clone, Debug)]
pub(crate) struct MirrorPaths {
    pub(crate) os_release: PathBuf,
    pub(crate) backup_root: PathBuf,
    pub(crate) apt_sources_list: PathBuf,
    pub(crate) apt_sources_list_d: PathBuf,
    pub(crate) dnf_repos_dir: PathBuf,
    pub(crate) pacman_mirrorlist: PathBuf,
    /// 恢复时把备份中的相对路径写回的根目录（生产为 `/`）。
    pub(crate) restore_root: PathBuf,
    /// 测试注入：允许非 root 执行换源/恢复（生产恒为 false）。
    #[cfg_attr(not(feature = "test-support"), allow(dead_code))]
    pub(crate) allow_non_root: bool,
}

impl MirrorPaths {
    pub(crate) fn production() -> Self {
        Self {
            os_release: PathBuf::from(OS_RELEASE_PATH),
            backup_root: PathBuf::from(BACKUP_ROOT),
            apt_sources_list: PathBuf::from(APT_SOURCES_LIST),
            apt_sources_list_d: PathBuf::from(APT_SOURCES_LIST_D),
            dnf_repos_dir: PathBuf::from(DNF_REPOS_DIR),
            pacman_mirrorlist: PathBuf::from(PACMAN_MIRRORLIST),
            restore_root: PathBuf::from("/"),
            allow_non_root: false,
        }
    }
}

/// 进程级路径配置。测试通过 [`test_support::TestPathsGuard`]（test-support）覆盖。
pub(crate) fn paths() -> &'static MirrorPaths {
    static PATHS: OnceLock<MirrorPaths> = OnceLock::new();
    #[cfg(all(test, feature = "test-support"))]
    {
        if let Some(overridden) = *test_support::TEST_PATHS
            .lock()
            .expect("mirror test paths lock poisoned")
        {
            return overridden;
        }
    }
    PATHS.get_or_init(MirrorPaths::production)
}

#[cfg(feature = "test-support")]
pub(crate) fn root_allowed() -> bool {
    paths().allow_non_root || unsafe { libc::geteuid() == 0 }
}

#[cfg(not(feature = "test-support"))]
pub(crate) fn root_allowed() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(all(test, feature = "test-support"))]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    use super::MirrorPaths;

    pub(crate) static TEST_PATHS: Mutex<Option<&'static MirrorPaths>> = Mutex::new(None);
    /// 所有 `TestPathsGuard` 使用者共享的串行锁：全局路径覆盖必须互斥，
    /// 否则并发测试会互相覆盖 `TEST_PATHS`。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 在测试作用域内覆盖 mirror 模块的路径与权限策略；Drop 时恢复生产配置。
    pub(crate) struct TestPathsGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl TestPathsGuard {
        pub(crate) fn set(paths: MirrorPaths) -> Self {
            let lock = TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let overridden: &'static MirrorPaths = Box::leak(Box::new(paths));
            *TEST_PATHS.lock().expect("mirror test paths lock poisoned") = Some(overridden);
            TestPathsGuard { _lock: lock }
        }
    }

    impl Drop for TestPathsGuard {
        fn drop(&mut self) {
            *TEST_PATHS.lock().expect("mirror test paths lock poisoned") = None;
        }
    }
}

/// 已识别的公共镜像主机。换源时除官方 URL 外，这些主机之间的 URL 也会互相转换；
/// 自定义内网镜像等未识别主机保持原样。
pub(crate) const RECOGNIZED_MIRROR_HOSTS: [&str; 9] = [
    "mirrors.tuna.tsinghua.edu.cn",
    "mirrors.aliyun.com",
    "mirrors.ustc.edu.cn",
    "mirror.nju.edu.cn",
    "mirror.sjtu.edu.cn",
    "mirrors.zju.edu.cn",
    "mirror.lzu.edu.cn",
    "mirrors.bfsu.edu.cn",
    "mirrors.hust.edu.cn",
];

/// 返回镜像主机；`Official` 没有镜像主机（反向映射回官方）。
pub(crate) fn mirror_host(mirror: MirrorName) -> Option<&'static str> {
    match mirror {
        MirrorName::Tuna => Some("mirrors.tuna.tsinghua.edu.cn"),
        MirrorName::Aliyun => Some("mirrors.aliyun.com"),
        MirrorName::Ustc => Some("mirrors.ustc.edu.cn"),
        MirrorName::Nju => Some("mirror.nju.edu.cn"),
        MirrorName::Sjtu => Some("mirror.sjtu.edu.cn"),
        MirrorName::Zju => Some("mirrors.zju.edu.cn"),
        MirrorName::Lzu => Some("mirror.lzu.edu.cn"),
        MirrorName::Bfsu => Some("mirrors.bfsu.edu.cn"),
        MirrorName::Hust => Some("mirrors.hust.edu.cn"),
        MirrorName::Official => None,
    }
}

/// 可切换的软件源镜像。
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum MirrorName {
    Tuna,
    Aliyun,
    Ustc,
    Nju,
    Sjtu,
    Zju,
    Lzu,
    Bfsu,
    Hust,
    Official,
}

impl MirrorName {
    pub(crate) const fn all() -> [Self; 10] {
        [
            Self::Tuna,
            Self::Aliyun,
            Self::Ustc,
            Self::Nju,
            Self::Sjtu,
            Self::Zju,
            Self::Lzu,
            Self::Bfsu,
            Self::Hust,
            Self::Official,
        ]
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Tuna => "tuna",
            Self::Aliyun => "aliyun",
            Self::Ustc => "ustc",
            Self::Nju => "nju",
            Self::Sjtu => "sjtu",
            Self::Zju => "zju",
            Self::Lzu => "lzu",
            Self::Bfsu => "bfsu",
            Self::Hust => "hust",
            Self::Official => "official",
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::Tuna => crate::tr!(crate::keys::mirror::MIRROR_TUNA),
            Self::Aliyun => crate::tr!(crate::keys::mirror::MIRROR_ALIYUN),
            Self::Ustc => crate::tr!(crate::keys::mirror::MIRROR_USTC),
            Self::Nju => crate::tr!(crate::keys::mirror::MIRROR_NJU),
            Self::Sjtu => crate::tr!(crate::keys::mirror::MIRROR_SJTU),
            Self::Zju => crate::tr!(crate::keys::mirror::MIRROR_ZJU),
            Self::Lzu => crate::tr!(crate::keys::mirror::MIRROR_LZU),
            Self::Bfsu => crate::tr!(crate::keys::mirror::MIRROR_BFSU),
            Self::Hust => crate::tr!(crate::keys::mirror::MIRROR_HUST),
            Self::Official => crate::tr!(crate::keys::mirror::MIRROR_OFFICIAL),
        }
    }
}

/// 发行版家族。决定软件源文件布局与 URL 重写规则。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Family {
    Debian,
    Ubuntu,
    Fedora,
    Rocky,
    Alma,
    Arch,
}

impl Family {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Debian => crate::tr!(crate::keys::mirror::FAMILY_DEBIAN),
            Self::Ubuntu => crate::tr!(crate::keys::mirror::FAMILY_UBUNTU),
            Self::Fedora => crate::tr!(crate::keys::mirror::FAMILY_FEDORA),
            Self::Rocky => crate::tr!(crate::keys::mirror::FAMILY_ROCKY),
            Self::Alma => crate::tr!(crate::keys::mirror::FAMILY_ALMA),
            Self::Arch => crate::tr!(crate::keys::mirror::FAMILY_ARCH),
        }
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Debian => "debian",
            Self::Ubuntu => "ubuntu",
            Self::Fedora => "fedora",
            Self::Rocky => "rocky",
            Self::Alma => "alma",
            Self::Arch => "arch",
        }
    }

    pub(crate) fn package_manager(self) -> &'static str {
        match self {
            Self::Debian | Self::Ubuntu => "apt",
            Self::Fedora | Self::Rocky | Self::Alma => "dnf",
            Self::Arch => "pacman",
        }
    }
}

/// 当前主机的发行版身份与版本代号（仅 apt 家族需要）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Host {
    pub family: Family,
    pub codename: Option<String>,
}

impl Host {
    /// 展示用摘要：`Debian (bookworm)` 或 `Fedora`。
    pub(crate) fn summary(&self) -> String {
        match &self.codename {
            Some(codename) => format!("{} ({codename})", self.family.label()),
            None => self.family.label(),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum MirrorError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 读取并检测当前主机的发行版家族。
pub(crate) fn detect_host() -> Result<Host, MirrorError> {
    detect::detect_from(&paths().os_release)
}

/// 列出当前主机可切换的镜像（公共镜像与官方源）。
pub(crate) fn list_mirrors() -> [MirrorName; 10] {
    MirrorName::all()
}

/// 显示当前软件源文件内容。仅支持当前发行版家族对应的源文件。
pub(crate) fn show_sources(host: &Host) -> Result<String, MirrorError> {
    backend::backend(host).show()
}

/// 把软件源切换到指定镜像。修改前备份原文件，备份目录见 [`BACKUP_ROOT`]。
/// `replace_security` 控制 Debian 独立 security 仓库是否一并替换（默认不替换）。
/// 返回本次修改与跳过的文件统计。
pub(crate) fn apply(
    host: &Host,
    mirror: MirrorName,
    replace_security: bool,
) -> Result<ApplyReport, MirrorError> {
    backend::backend(host).apply(mirror, replace_security)
}

/// 从 [`BACKUP_ROOT`] 恢复原软件源，成功后删除备份。
pub(crate) fn restore(host: &Host) -> Result<(), MirrorError> {
    backend::backend(host).restore()
}

/// 只读检查当前软件源文件的格式（仅 apt 家族有诊断；其余家族返回空）。
/// 返回每个含问题的文件与其异常行列表。
pub(crate) fn check_format(
    host: &Host,
) -> Result<Vec<(PathBuf, Vec<apt::parse::ParseIssue>)>, MirrorError> {
    match host.family {
        Family::Debian | Family::Ubuntu => apt::check_format(),
        _ => Ok(Vec::new()),
    }
}

/// 一次换源操作的统计结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ApplyReport {
    pub changed_files: usize,
    pub skipped_repositories: usize,
    pub backup_path: Option<PathBuf>,
    /// 常规 URL 重写之外的兜底路径（仅 apt 家族可能产生）。
    pub fallback: Option<Fallback>,
    /// apt：格式检查发现的无法识别行数（仅诊断，这些行已原样保留）。
    pub unrecognized_lines: usize,
}

/// apt 换源的兜底方式：源文件中没有任何可识别 URL 时采用。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Fallback {
    /// 把现有的 `deb cdrom:` 条目转换为所选镜像（保留其 suites/components）。
    CdromConverted,
    /// 用检测到的代号合成新源条目并追加。
    SourceAdded,
}

/// 备份目录：`<backup_root>/<family-id>/`。
pub(crate) fn backup_dir(family: Family) -> PathBuf {
    paths().backup_root.join(family.id())
}
