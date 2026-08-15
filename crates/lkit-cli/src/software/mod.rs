pub(crate) mod detect;
pub(crate) mod docker;

use std::path::PathBuf;
use std::sync::OnceLock;

use clap::ValueEnum;
use thiserror::Error;

use crate::mirror::{Family, Host};

/// 生产环境的系统文件根路径。
pub(crate) const OS_RELEASE_PATH: &str = "/etc/os-release";
pub(crate) const APT_KEYRINGS_DIR: &str = "/etc/apt/keyrings";
pub(crate) const APT_SOURCES_LIST_D: &str = "/etc/apt/sources.list.d";
pub(crate) const DNF_REPOS_DIR: &str = "/etc/yum.repos.d";

/// 常用软件。新增软件时在此添加变体并实现对应后端。
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Hash)]
pub(crate) enum Software {
    /// Docker 容器引擎
    Docker,
}

impl Software {
    pub(crate) fn all() -> [Self; 1] {
        [Self::Docker]
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Docker => "docker",
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::Docker => crate::tr!(crate::keys::SOFTWARE_DOCKER),
        }
    }

    /// 当前主机上是否已安装（只读检测）。
    pub(crate) fn installed(&self) -> bool {
        match self {
            Self::Docker => detect::docker_installed(),
        }
    }
}

/// Docker 的安装来源：官方仓库或国内镜像。
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum DockerSource {
    /// 官方仓库
    Official,
    /// 阿里云镜像
    Aliyun,
    /// 清华 TUNA 镜像
    Tuna,
    /// 中科大 USTC 镜像
    Ustc,
}

impl DockerSource {
    pub(crate) fn all() -> [Self; 4] {
        [Self::Official, Self::Aliyun, Self::Tuna, Self::Ustc]
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::Official => crate::tr!(crate::keys::SOFTWARE_SOURCE_OFFICIAL),
            Self::Aliyun => crate::tr!(crate::keys::SOFTWARE_SOURCE_ALIYUN),
            Self::Tuna => crate::tr!(crate::keys::SOFTWARE_SOURCE_TUNA),
            Self::Ustc => crate::tr!(crate::keys::SOFTWARE_SOURCE_USTC),
        }
    }

    /// 各来源的 docker-ce 仓库根 URL。
    pub(crate) fn base_url(self) -> &'static str {
        match self {
            Self::Official => "https://download.docker.com",
            Self::Aliyun => "https://mirrors.aliyun.com/docker-ce",
            Self::Tuna => "https://mirrors.tuna.tsinghua.edu.cn/docker-ce",
            Self::Ustc => "https://mirrors.ustc.edu.cn/docker-ce",
        }
    }
}

/// 安装流程的阶段，用于进度展示。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallPhase {
    Preparing,
    InstallingPackages,
    StartingService,
}

impl InstallPhase {
    pub(crate) fn step(self) -> u8 {
        match self {
            Self::Preparing => 1,
            Self::InstallingPackages => 2,
            Self::StartingService => 3,
        }
    }

    pub(crate) const STEPS: u8 = 3;
}

/// software 模块读写的主机路径与权限策略。生产环境恒为 [`SoftwarePaths::production`]；
/// `test-support` 构建下测试可以覆盖为临时目录，隔离真实系统文件。
#[derive(Clone, Debug)]
pub(crate) struct SoftwarePaths {
    pub(crate) os_release: PathBuf,
    pub(crate) apt_keyrings_dir: PathBuf,
    pub(crate) apt_sources_list_d: PathBuf,
    pub(crate) dnf_repos_dir: PathBuf,
    pub(crate) docker_bin: Vec<PathBuf>,
    /// 测试注入：允许非 root 执行安装（生产恒为 false）。
    #[cfg_attr(not(feature = "test-support"), allow(dead_code))]
    pub(crate) allow_non_root: bool,
}

impl SoftwarePaths {
    pub(crate) fn production() -> Self {
        Self {
            os_release: PathBuf::from(OS_RELEASE_PATH),
            apt_keyrings_dir: PathBuf::from(APT_KEYRINGS_DIR),
            apt_sources_list_d: PathBuf::from(APT_SOURCES_LIST_D),
            dnf_repos_dir: PathBuf::from(DNF_REPOS_DIR),
            docker_bin: [
                "/usr/bin/docker",
                "/usr/local/bin/docker",
                "/bin/docker",
                "/usr/sbin/docker",
                "/usr/local/sbin/docker",
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
            allow_non_root: false,
        }
    }
}

/// 进程级路径配置。测试通过 [`test_support::TestPathsGuard`]（test-support）覆盖。
pub(crate) fn paths() -> &'static SoftwarePaths {
    static PATHS: OnceLock<SoftwarePaths> = OnceLock::new();
    #[cfg(all(test, feature = "test-support"))]
    {
        if let Some(overridden) = *test_support::TEST_PATHS
            .lock()
            .expect("software test paths lock poisoned")
        {
            return overridden;
        }
    }
    PATHS.get_or_init(SoftwarePaths::production)
}

#[cfg(feature = "test-support")]
pub(crate) fn root_allowed() -> bool {
    paths().allow_non_root || unsafe { libc::geteuid() == 0 }
}

#[cfg(not(feature = "test-support"))]
pub(crate) fn root_allowed() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// 安装一个常用软件。`stream` 为 true 时软件包管理器输出直接流到终端
/// （CLI 模式）；false 时捕获输出，仅错误时透出（TUI worker 模式）。
/// `phase` 回调按流程阶段上报进度。
pub(crate) fn install(
    host: &Host,
    software: Software,
    source: DockerSource,
    stream: bool,
    phase: &mut dyn FnMut(InstallPhase),
) -> Result<(), SoftwareError> {
    match software {
        Software::Docker => docker::install(host, source, stream, phase),
    }
}

#[derive(Debug, Error)]
pub(crate) enum SoftwareError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(all(test, feature = "test-support"))]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    use super::SoftwarePaths;

    pub(crate) static TEST_PATHS: Mutex<Option<&'static SoftwarePaths>> = Mutex::new(None);
    /// 所有 `TestPathsGuard` 使用者共享的串行锁：全局路径覆盖必须互斥，
    /// 否则并发测试会互相覆盖 `TEST_PATHS`。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 在测试作用域内覆盖 software 模块的路径与权限策略；Drop 时恢复生产配置。
    pub(crate) struct TestPathsGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl TestPathsGuard {
        pub(crate) fn set(paths: SoftwarePaths) -> Self {
            let lock = TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let overridden: &'static SoftwarePaths = Box::leak(Box::new(paths));
            *TEST_PATHS
                .lock()
                .expect("software test paths lock poisoned") = Some(overridden);
            TestPathsGuard { _lock: lock }
        }
    }

    impl Drop for TestPathsGuard {
        fn drop(&mut self) {
            *TEST_PATHS
                .lock()
                .expect("software test paths lock poisoned") = None;
        }
    }
}

/// 家族在 docker-ce 仓库路径中的目录名（apt/dnf 共用布局）。
pub(crate) fn docker_family_slug(family: Family) -> &'static str {
    match family {
        Family::Debian => "debian",
        Family::Ubuntu => "ubuntu",
        Family::Fedora => "fedora",
        Family::Centos7 => "centos",
        Family::CentosStream => "centos-stream",
        Family::Rocky => "rocky",
        Family::Alma => "almalinux",
        Family::Arch => "arch",
    }
}
