use std::path::Path;

use super::{Family, Host, MirrorError};

/// 解析 `/etc/os-release` 的 `ID` 字段。引号被剥离，无引号值直接使用。
fn os_release_id(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("ID=")?;
        let id = rest.trim().trim_matches('"');
        (!id.is_empty()).then(|| id.to_string())
    })
}

/// 解析 `/etc/os-release` 的版本字段：优先 `VERSION_CODENAME`，其次 `VERSION`。
fn os_release_codename(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let codename = content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("VERSION_CODENAME=")?;
        let value = rest.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    });
    if codename.is_some() {
        return codename;
    }
    let version = content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("VERSION=")?;
        let value = rest.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    });
    version.and_then(|value| {
        // "24.04.1 LTS (Noble Numbat)" -> noble：去掉标点后取纯字母、长度 > 3 的单词。
        value
            .split_whitespace()
            .map(|word| {
                word.chars()
                    .filter(|character| character.is_ascii_alphabetic())
                    .collect::<String>()
                    .to_lowercase()
            })
            .find(|word| !word.is_empty() && word.len() > 3)
    })
}

/// 从 os-release 内容检测发行版家族与版本代号。
pub(crate) fn detect_from(path: &Path) -> Result<Host, MirrorError> {
    let id = os_release_id(path).ok_or_else(|| {
        MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_OS_RELEASE_UNREADABLE,
            path = path.display()
        ))
    })?;
    let family = match id.as_str() {
        "debian" => Family::Debian,
        "ubuntu" => Family::Ubuntu,
        "fedora" => Family::Fedora,
        "centos" => {
            // CentOS 7 使用 yum/旧仓库布局，Stream 8+ 使用新布局。
            let version = os_release_version(path).unwrap_or_default();
            if version.starts_with('7') {
                Family::Centos7
            } else {
                Family::CentosStream
            }
        }
        "rocky" => Family::Rocky,
        "almalinux" => Family::Alma,
        "arch" => Family::Arch,
        _ => {
            return Err(MirrorError::Message(crate::tr!(
                crate::keys::mirror::MIRROR_DISTRO_UNSUPPORTED,
                id = id
            )));
        }
    };
    let codename = match family {
        Family::Debian | Family::Ubuntu => os_release_codename(path),
        _ => None,
    };
    Ok(Host { family, codename })
}

fn os_release_version(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("VERSION_ID=")?;
        let value = rest.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_os_release(content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-mirror-detect-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("os-release");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    #[test]
    fn detects_debian_with_codename() {
        let path = write_os_release(
            "ID=debian\nVERSION_ID=\"12\"\nVERSION_CODENAME=bookworm\nVERSION=\"12 (bookworm)\"\n",
        );
        assert_eq!(
            detect_from(&path).unwrap(),
            Host {
                family: Family::Debian,
                codename: Some("bookworm".into()),
            }
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn detects_ubuntu_from_version_line_when_codename_missing() {
        let path = write_os_release("ID=ubuntu\nVERSION=\"24.04.1 LTS (Noble Numbat)\"\n");
        let host = detect_from(&path).unwrap();
        assert_eq!(host.family, Family::Ubuntu);
        assert_eq!(host.codename.as_deref(), Some("noble"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn detects_centos7_by_version_prefix() {
        let path = write_os_release("ID=\"centos\"\nVERSION_ID=\"7\"\n");
        assert_eq!(detect_from(&path).unwrap().family, Family::Centos7);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn detects_centos_stream_without_version() {
        let path = write_os_release("ID=\"centos\"\nVERSION_ID=\"9\"\n");
        assert_eq!(detect_from(&path).unwrap().family, Family::CentosStream);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn detects_other_families() {
        for (id, expected) in [
            ("fedora", Family::Fedora),
            ("rocky", Family::Rocky),
            ("almalinux", Family::Alma),
            ("arch", Family::Arch),
        ] {
            let path = write_os_release(&format!("ID={id}\n"));
            assert_eq!(detect_from(&path).unwrap().family, expected);
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    #[test]
    fn rejects_unsupported_distributions() {
        let path = write_os_release("ID=alpine\n");
        assert!(detect_from(&path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rejects_unreadable_os_release() {
        let path = write_os_release("");
        std::fs::remove_file(&path).unwrap();
        assert!(detect_from(&path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
