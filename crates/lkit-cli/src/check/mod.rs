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
        ("运行身份与平台", platform::run()),
        ("内核版本与内核能力", kernel::run()),
        ("资源限制", resource::run()),
        ("必需命令与运行时依赖", dependency::run()),
        ("端口冲突", ports::run()),
        ("系统服务与安全策略", service::run()),
        ("DNS 配置风险", dns::run()),
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
