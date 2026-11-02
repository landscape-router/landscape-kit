//! OpenRC-specific requirements selected by the common init check domain.
use super::model::{CheckResult, Status};

pub(super) fn run(boot_enabled: impl FnOnce() -> bool) -> Vec<CheckResult> {
    let result = CheckResult::new(
        "openrc.cgroup_boot",
        crate::tr!(crate::keys::OPENRC_CGROUP_BOOT),
    );
    vec![if boot_enabled() {
        result.set(
            Status::Pass,
            "boot",
            crate::tr!(crate::keys::OPENRC_CGROUP_BOOT_ENABLED),
        )
    } else {
        result
            .set(
                Status::Error,
                "not enabled",
                crate::tr!(crate::keys::OPENRC_CGROUP_BOOT_REQUIRED),
            )
            .suggestion("rc-update add cgroups boot")
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_registration_has_its_own_domain_and_is_required() {
        for (enabled, status) in [(true, Status::Pass), (false, Status::Error)] {
            let results = crate::i18n::with_language(crate::i18n::Language::En, || run(|| enabled));
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].id, "openrc.cgroup_boot");
            assert_eq!(results[0].status, status);
            assert_eq!(results[0].title, "Cgroup boot service");
        }
    }
}
