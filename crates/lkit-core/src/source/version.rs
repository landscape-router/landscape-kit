//! Version tag comparison utilities.

use std::cmp::Ordering;

/// Compare two semver-style tags (e.g. "v1.2.3") component by component.
///
/// Returns `Greater` if `a` is a newer version than `b`.
pub fn compare_semver(a: &str, b: &str) -> Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|c| c.parse::<u64>().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for (ca, cb) in va.iter().zip(vb.iter()) {
        match ca.cmp(cb) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    va.len().cmp(&vb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_major_version() {
        assert_eq!(compare_semver("v2.0", "v1.0"), Ordering::Greater);
        assert_eq!(compare_semver("v1.0", "v2.0"), Ordering::Less);
    }

    #[test]
    fn compare_minor_version() {
        assert_eq!(compare_semver("v0.19.2", "v0.9.0"), Ordering::Greater);
        assert_eq!(compare_semver("v0.9.0", "v0.19.2"), Ordering::Less);
    }

    #[test]
    fn compare_equal() {
        assert_eq!(compare_semver("v1.0", "v1.0"), Ordering::Equal);
    }

    #[test]
    fn compare_different_lengths() {
        assert_eq!(compare_semver("v1.0.1", "v1.0"), Ordering::Greater);
    }

    #[test]
    fn compare_no_v_prefix() {
        assert_eq!(compare_semver("2.0", "1.0"), Ordering::Greater);
    }
}
