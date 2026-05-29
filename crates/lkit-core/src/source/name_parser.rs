//! Parse architecture and libc variant from artifact filenames.

/// Extracted architecture info from an artifact filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchInfo {
    /// Architecture identifier (e.g. "x86_64", "aarch64").
    pub arch: String,
    /// Whether the artifact uses musl libc.
    pub musl: bool,
}

const KNOWN_ARCHES: &[&str] = &["x86_64", "aarch64", "loongarch64", "riscv64", "s390x"];

/// Parse architecture from a filename like `landscape-webserver-x86_64-musl`.
///
/// Returns `None` for arch-independent files like `static.zip` or `SHASUM256sum.txt`.
pub fn parse_arch(filename: &str) -> Option<ArchInfo> {
    // Strip file extension
    let stem = filename.rsplitn(2, '.').last().unwrap_or(filename);

    // Check musl suffix
    let (stem, musl) = if let Some(s) = stem.strip_suffix("-musl") {
        (s, true)
    } else {
        (stem, false)
    };

    // Match known arch at the end of the stem
    for arch in KNOWN_ARCHES {
        if let Some(_remaining) = stem.strip_suffix(&format!("-{arch}")) {
            return Some(ArchInfo {
                arch: arch.to_string(),
                musl,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_landscape_webserver_x86_64() {
        let info = parse_arch("landscape-webserver-x86_64")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-x86_64"));
        assert_eq!(info.arch, "x86_64");
        assert!(!info.musl);
    }

    #[test]
    fn parse_landscape_webserver_x86_64_musl() {
        let info = parse_arch("landscape-webserver-x86_64-musl")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-x86_64-musl"));
        assert_eq!(info.arch, "x86_64");
        assert!(info.musl);
    }

    #[test]
    fn parse_landscape_webserver_aarch64() {
        let info = parse_arch("landscape-webserver-aarch64")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-aarch64"));
        assert_eq!(info.arch, "aarch64");
        assert!(!info.musl);
    }

    #[test]
    fn parse_landscape_webserver_riscv64() {
        let info = parse_arch("landscape-webserver-riscv64")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-riscv64"));
        assert_eq!(info.arch, "riscv64");
        assert!(!info.musl);
    }

    #[test]
    fn parse_redirect_pkg_x86_64() {
        let info = parse_arch("redirect_pkg_handler-x86_64")
            .unwrap_or_else(|| panic!("expected Some for redirect_pkg_handler-x86_64"));
        assert_eq!(info.arch, "x86_64");
    }

    #[test]
    fn parse_redirect_pkg_x86_64_musl() {
        let info = parse_arch("redirect_pkg_handler-x86_64-musl")
            .unwrap_or_else(|| panic!("expected Some for redirect_pkg_handler-x86_64-musl"));
        assert_eq!(info.arch, "x86_64");
        assert!(info.musl);
    }

    #[test]
    fn parse_static_zip_returns_none() {
        assert!(parse_arch("static.zip").is_none());
    }

    #[test]
    fn parse_shasum_returns_none() {
        assert!(parse_arch("SHASUM256sum.txt").is_none());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_arch("").is_none());
    }

    #[test]
    fn parse_no_arch_returns_none() {
        assert!(parse_arch("somefile").is_none());
    }

    #[test]
    fn parse_unknown_arch_returns_none() {
        assert!(parse_arch("binary-mips").is_none());
    }

    #[test]
    fn parse_loongarch64() {
        let info = parse_arch("landscape-webserver-loongarch64")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-loongarch64"));
        assert_eq!(info.arch, "loongarch64");
        assert!(!info.musl);
    }

    #[test]
    fn parse_s390x() {
        let info = parse_arch("landscape-webserver-s390x")
            .unwrap_or_else(|| panic!("expected Some for landscape-webserver-s390x"));
        assert_eq!(info.arch, "s390x");
        assert!(!info.musl);
    }
}
