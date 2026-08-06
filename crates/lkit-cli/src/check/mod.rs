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
            crate::tr!(crate::keys::CHECK_RUNTIME_IDENTITY_AND_PLATFORM),
            platform::run(),
        ),
        (
            crate::tr!(crate::keys::CHECK_KERNEL_VERSION_AND_CAPABILITIES),
            kernel::run(),
        ),
        (
            crate::tr!(crate::keys::CHECK_RESOURCE_LIMITS),
            resource::run(),
        ),
        (
            crate::tr!(crate::keys::CHECK_REQUIRED_COMMANDS_AND_RUNTIME_DEPENDENCIES),
            dependency::run(),
        ),
        (crate::tr!(crate::keys::CHECK_PORT_CONFLICTS), ports::run()),
        (
            crate::tr!(crate::keys::CHECK_SYSTEM_SERVICES_AND_SECURITY_POLICY),
            service::run(),
        ),
        (
            crate::tr!(crate::keys::CHECK_DNS_CONFIGURATION_RISKS),
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
