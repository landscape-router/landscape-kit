use super::model::{CheckResult, Status};

const MIN_TOTAL_MEMORY_KIB: u64 = 2 * 1024 * 1024;

pub fn run() -> Vec<CheckResult> {
    vec![memory()]
}

fn memory() -> CheckResult {
    let result = CheckResult::new("resource.memory", crate::tr!("Host memory", "主机内存"));
    let Some(meminfo) = std::fs::read_to_string("/proc/meminfo").ok() else {
        return result.set(
            Status::Unknown,
            crate::tr!("unavailable", "无法读取"),
            crate::tr!("Unable to read /proc/meminfo", "无法读取 /proc/meminfo"),
        );
    };
    let total_kib = meminfo_value(&meminfo, "MemTotal");
    let available_kib = meminfo_value(&meminfo, "MemAvailable");
    let total_mib = total_kib.map(|kib| kib / 1024);
    let available_mib = available_kib.map(|kib| kib / 1024);
    let value = match (total_mib, available_mib) {
        (Some(total), Some(available)) => crate::trf!(
            ("total {total} MiB, available {available} MiB"),
            ("总内存 {total} MiB，可用 {available} MiB")
        ),
        (Some(total), None) => crate::trf!(
            ("total {total} MiB (available memory unknown)"),
            ("总内存 {total} MiB（可用内存未知）")
        ),
        _ => crate::tr!("memory information unavailable", "内存信息不可用").to_string(),
    };
    match total_kib {
        Some(total) if total >= MIN_TOTAL_MEMORY_KIB => result.set(
            Status::Pass,
            value,
            crate::tr!("Memory meets the 2 GiB minimum", "内存满足 2 GiB 最低要求"),
        ),
        Some(total) => result
            .set(
                Status::Error,
                value,
                crate::trf!(
                    ("Total memory is below the 2 GiB minimum (current: {total} KiB)"),
                    ("总内存低于 2 GiB 最低要求（当前 {total} KiB）")
                ),
            )
            .suggestion(crate::tr!(
                "Increase host memory to at least 2 GiB",
                "增加主机内存到至少 2 GiB"
            )),
        None => result.set(
            Status::Unknown,
            value,
            crate::tr!("Unable to read MemTotal", "无法读取 MemTotal"),
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
