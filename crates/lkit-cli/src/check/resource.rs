use super::model::{CheckResult, Status};

const MIN_TOTAL_MEMORY_KIB: u64 = 2 * 1024 * 1024;

pub fn run() -> Vec<CheckResult> {
    vec![memory()]
}

fn memory() -> CheckResult {
    let result = CheckResult::new(
        "resource.memory",
        crate::tr!(crate::keys::RESOURCE_HOST_MEMORY),
    );
    let Some(meminfo) = std::fs::read_to_string("/proc/meminfo").ok() else {
        return result.set(
            Status::Unknown,
            crate::tr!(crate::keys::RESOURCE_UNAVAILABLE),
            crate::tr!(crate::keys::RESOURCE_UNABLE_READ_MEMINFO),
        );
    };
    let total_kib = meminfo_value(&meminfo, "MemTotal");
    let available_kib = meminfo_value(&meminfo, "MemAvailable");
    let total_mib = total_kib.map(|kib| kib / 1024);
    let available_mib = available_kib.map(|kib| kib / 1024);
    let value = match (total_mib, available_mib) {
        (Some(total), Some(available)) => crate::tr!(
            crate::keys::RESOURCE_TOTAL_AVAILABLE_MEMORY,
            total = total,
            available = available
        ),
        (Some(total), None) => crate::tr!(
            crate::keys::RESOURCE_TOTAL_MEMORY_AVAILABLE_UNKNOWN,
            total = total
        ),
        _ => crate::tr!(crate::keys::RESOURCE_MEMORY_INFORMATION_UNAVAILABLE),
    };
    match total_kib {
        Some(total) if total >= MIN_TOTAL_MEMORY_KIB => result.set(
            Status::Pass,
            value,
            crate::tr!(crate::keys::RESOURCE_MEMORY_MEETS_2GIB_MINIMUM),
        ),
        Some(total) => result
            .set(
                Status::Error,
                value,
                crate::tr!(crate::keys::RESOURCE_TOTAL_MEMORY_BELOW_2GIB, total = total),
            )
            .suggestion(crate::tr!(crate::keys::RESOURCE_INCREASE_MEMORY_TO_2GIB)),
        None => result.set(
            Status::Unknown,
            value,
            crate::tr!(crate::keys::RESOURCE_UNABLE_READ_MEMTOTAL),
        ),
    }
}

fn meminfo_value(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        let value = rest.trim_start_matches(':').trim();
        value.split_whitespace().next()?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_values() {
        let meminfo = "MemTotal:       16289768 kB\nMemAvailable:    7012345 kB\nSwapTotal:       2097152 kB\n";
        assert_eq!(meminfo_value(meminfo, "MemTotal"), Some(16289768));
        assert_eq!(meminfo_value(meminfo, "MemAvailable"), Some(7012345));
        assert_eq!(meminfo_value(meminfo, "MemFree"), None);
    }
}
