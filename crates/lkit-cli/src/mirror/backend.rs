use super::{ApplyReport, Family, Host, MirrorError, MirrorName, apt, dnf, pacman};

/// 各发行版软件源后端的统一接口。
///
/// 每个家族（apt/dnf/pacman）实现该 trait；[`backend`] 按检测出的 `Host` 构造
/// 对应后端。上层（`mirror::show_sources`/`apply`/`restore`）只面对这个 trait，
/// 不再按家族分支。
pub(crate) trait SourcesBackend {
    /// 显示当前软件源文件内容。
    fn show(&self) -> Result<String, MirrorError>;

    /// 切换到指定镜像。`replace_security` 仅 Debian 家族生效（是否一并替换
    /// 独立 security 仓库），其余家族忽略该参数。
    fn apply(&self, mirror: MirrorName, replace_security: bool)
    -> Result<ApplyReport, MirrorError>;

    /// 从上次换源的备份恢复原软件源，成功后删除备份。
    fn restore(&self) -> Result<(), MirrorError>;
}

/// 按发行版家族构造后端；家族在检测阶段已确认，构造不会失败。
pub(crate) fn backend(host: &Host) -> Box<dyn SourcesBackend> {
    match host.family {
        Family::Debian | Family::Ubuntu => Box::new(apt::AptBackend::new(host)),
        Family::Fedora | Family::Rocky | Family::Alma => Box::new(dnf::DnfBackend::new(host)),
        Family::Arch => Box::new(pacman::PacmanBackend),
    }
}
