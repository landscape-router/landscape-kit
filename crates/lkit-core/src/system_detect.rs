//! Detect the host system's CPU architecture and libc variant at runtime.

use std::process::Command;

use crate::error::CoreError;

/// The host system's CPU architecture and libc type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTarget {
    /// CPU architecture (e.g. "x86_64", "aarch64").
    pub arch: String,
    /// Detected libc variant.
    pub libc: LibcType,
    /// Matching string for artifact filenames, e.g. "x86_64" or "x86_64-musl".
    pub target_str: String,
}

/// C library variant detected on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibcType {
    /// GNU C Library.
    Glibc,
    /// musl libc.
    Musl,
}

/// Detect the current system's target (arch + libc).
///
/// Returns an error only on x86_64 when neither glibc nor musl can be detected.
/// On other architectures, falls back to glibc if detection fails.
pub fn detect() -> Result<SystemTarget, CoreError> {
    let arch = map_arch(std::env::consts::ARCH)?;
    let libc = detect_libc(&arch)?;
    let target_str = if libc == LibcType::Musl {
        format!("{arch}-musl")
    } else {
        arch.clone()
    };
    Ok(SystemTarget {
        arch,
        libc,
        target_str,
    })
}

fn map_arch(rust_arch: &str) -> Result<String, CoreError> {
    match rust_arch {
        "x86_64" => Ok("x86_64".into()),
        "aarch64" => Ok("aarch64".into()),
        "riscv64" | "riscv64gc" => Ok("riscv64".into()),
        "s390x" => Ok("s390x".into()),
        "loongarch64" => Ok("loongarch64".into()),
        other => Err(CoreError::Internal(format!(
            "unsupported architecture: {other}"
        ))),
    }
}

fn detect_libc(arch: &str) -> Result<LibcType, CoreError> {
    // Try glibc linker paths for this arch
    let glibc_paths: &[&str] = match arch {
        "x86_64" => &["/lib64/ld-linux-x86-64.so.2"],
        "aarch64" => &[
            "/lib/ld-linux-aarch64.so.1",
            "/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
            "/usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
        ],
        "riscv64" => &[
            "/lib/ld-linux-riscv64-lp64d.so.1",
            "/usr/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1",
        ],
        "s390x" => &[
            "/lib/ld-linux-s390x.so.1",
            "/usr/lib/s390x-linux-gnu/ld-linux-s390x.so.1",
        ],
        "loongarch64" => &[
            "/lib/ld-linux-loongarch64-lp64d.so.1",
            "/usr/lib/loongarch64-linux-gnu/ld-linux-loongarch64-lp64d.so.1",
        ],
        _ => &[],
    };

    for path in glibc_paths {
        if let Ok(output) = Command::new(path).arg("--version").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("GLIBC") || stdout.contains("GNU libc") {
                return Ok(LibcType::Glibc);
            }
        }
    }

    // Try musl (only x86_64 has musl variant in practice)
    if arch == "x86_64" {
        let output = Command::new("/lib/ld-musl-x86_64.so.1")
            .arg("--version")
            .output()
            .map_err(|_| {
                CoreError::Internal("unable to detect libc (tried glibc and musl)".into())
            })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("musl libc") {
            return Ok(LibcType::Musl);
        }
    }

    // Non-x86_64: if glibc probe failed, assume glibc (only glibc variants exist)
    if arch != "x86_64" {
        return Ok(LibcType::Glibc);
    }

    Err(CoreError::Internal(
        "unable to detect libc on x86_64".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_arch_x86_64() {
        assert_eq!(
            map_arch("x86_64").unwrap_or_else(|e| panic!("{e}")),
            "x86_64"
        );
    }

    #[test]
    fn map_arch_riscv64gc() {
        assert_eq!(
            map_arch("riscv64gc").unwrap_or_else(|e| panic!("{e}")),
            "riscv64"
        );
    }

    #[test]
    fn map_arch_unknown_fails() {
        assert!(map_arch("mips").is_err());
    }

    #[test]
    fn detect_returns_valid_target() -> Result<(), Box<dyn std::error::Error>> {
        let target = detect()?;
        assert!(!target.arch.is_empty());
        assert!(!target.target_str.is_empty());
        Ok(())
    }

    #[test]
    fn target_str_musl_has_suffix() {
        let t = SystemTarget {
            arch: "x86_64".into(),
            libc: LibcType::Musl,
            target_str: "x86_64-musl".into(),
        };
        assert!(t.target_str.contains("-musl"));
    }

    #[test]
    fn target_str_glibc_no_suffix() {
        let t = SystemTarget {
            arch: "x86_64".into(),
            libc: LibcType::Glibc,
            target_str: "x86_64".into(),
        };
        assert!(!t.target_str.contains("-musl"));
    }
}
