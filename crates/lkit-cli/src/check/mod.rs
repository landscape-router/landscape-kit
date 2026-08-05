pub mod dependency;
pub mod dns;
pub mod kernel;
pub mod model;
pub mod platform;
pub mod ports;
pub mod resource;
pub mod service;

use model::{CheckGroup, CheckReport, Status, StatusCounts, aggregate_status};

pub fn run_all() -> CheckReport {
    let mut counts = StatusCounts::default();
    let mut groups = Vec::new();
    for (title, results) in [
        (
            crate::tr!("Runtime identity and platform", "运行身份与平台"),
            platform::run(),
        ),
        (
            crate::tr!("Kernel version and capabilities", "内核版本与内核能力"),
            kernel::run(),
        ),
        (crate::tr!("Resource limits", "资源限制"), resource::run()),
        (
            crate::tr!(
                "Required commands and runtime dependencies",
                "必需命令与运行时依赖"
            ),
            dependency::run(),
        ),
        (crate::tr!("Port conflicts", "端口冲突"), ports::run()),
        (
            crate::tr!("System services and security policy", "系统服务与安全策略"),
            service::run(),
        ),
        (
            crate::tr!("DNS configuration risks", "DNS 配置风险"),
            dns::run(),
        ),
    ] {
        for result in &results {
            match result.status {
                Status::Pass => counts.pass += 1,
                Status::Warning => counts.warning += 1,
                Status::Error => counts.error += 1,
                Status::Unknown => counts.unknown += 1,
            }
        }
        groups.push(CheckGroup { title, results });
    }
    let summary = aggregate_status(
        groups
            .iter()
            .flat_map(|group| group.results.iter())
            .map(|result| result.status),
    );
    CheckReport {
        groups,
        summary,
        counts,
    }
}
