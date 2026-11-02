use std::fs;

use super::backend::SourcesBackend;
use super::{
    ApplyReport, Family, MirrorError, MirrorName, RECOGNIZED_MIRROR_HOSTS, backup_dir, common,
    mirror_host, paths,
};

/// Alpine repositories 文件后端；仅改写已识别的官方/公共镜像 URL。
pub(crate) struct ApkBackend;

const OFFICIAL_HOST: &str = "dl-cdn.alpinelinux.org";

fn recognized_host(host: &str) -> bool {
    host == OFFICIAL_HOST
        || RECOGNIZED_MIRROR_HOSTS.contains(&host)
        || matches!(
            host,
            "dl-1.alpinelinux.org"
                | "dl-2.alpinelinux.org"
                | "dl-3.alpinelinux.org"
                | "dl-4.alpinelinux.org"
                | "dl-5.alpinelinux.org"
        )
}

/// 保留版本/edge、仓库组件、@tag、缩进与注释。自定义源和本地路径不改写。
fn rewrite(content: &str, mirror: MirrorName) -> (String, usize) {
    let mut output = String::with_capacity(content.len());
    let mut skipped = 0;
    for line in content.split_inclusive('\n') {
        let active = line.split('#').next().unwrap_or_default();
        let mut tokens = active.split_whitespace();
        let Some(first) = tokens.next() else {
            output.push_str(line);
            continue;
        };
        let repository = if first.starts_with('@') {
            tokens.next().unwrap_or_default()
        } else {
            first
        };
        let replacement = (|| {
            if tokens.next().is_some() {
                return None;
            }
            let url = url::Url::parse(repository).ok()?;
            if !matches!(url.scheme(), "http" | "https")
                || !recognized_host(url.host_str()?)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return None;
            }
            let suffix = url.path().strip_prefix("/alpine/")?;
            Some(format!(
                "https://{}/alpine/{suffix}",
                mirror_host(mirror).unwrap_or(OFFICIAL_HOST)
            ))
        })();
        match replacement {
            Some(url) => output.push_str(&line.replacen(repository, &url, 1)),
            None => {
                skipped += 1;
                output.push_str(line);
            }
        }
    }
    (output, skipped)
}

impl SourcesBackend for ApkBackend {
    fn show(&self) -> Result<String, MirrorError> {
        let path = &paths().apk_repositories;
        if !path.is_file() {
            return Err(MirrorError::Message(crate::tr!(
                crate::keys::mirror::MIRROR_NO_SOURCE_FILES
            )));
        }
        Ok(format!(
            "# {}\n{}",
            path.display(),
            fs::read_to_string(path)?
        ))
    }

    fn apply(
        &self,
        mirror: MirrorName,
        _replace_security: bool,
        _disable_cdrom: bool,
    ) -> Result<ApplyReport, MirrorError> {
        let path = &paths().apk_repositories;
        if !path.is_file() {
            return Err(MirrorError::Message(crate::tr!(
                crate::keys::mirror::MIRROR_NO_SOURCE_FILES
            )));
        }
        let original = fs::read_to_string(path)?;
        let (content, skipped_repositories) = rewrite(&original, mirror);
        if content == original {
            return Ok(ApplyReport {
                skipped_repositories,
                ..ApplyReport::default()
            });
        }
        let dir = backup_dir(Family::Alpine);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        let backup = dir.join("etc/apk/repositories");
        fs::create_dir_all(backup.parent().expect("backup has a parent"))?;
        fs::copy(path, &backup)?;
        if let Err(error) = common::write_atomic(path, &content) {
            let _ = common::rollback(&dir);
            return Err(error);
        }
        Ok(ApplyReport {
            changed_files: 1,
            skipped_repositories,
            backup_path: Some(dir),
            ..ApplyReport::default()
        })
    }

    fn restore(&self) -> Result<(), MirrorError> {
        let dir = backup_dir(Family::Alpine);
        if !dir.exists() {
            return Err(MirrorError::Message(crate::tr!(
                crate::keys::mirror::MIRROR_NO_BACKUP
            )));
        }
        common::restore_files(&dir, &paths().restore_root)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_all_mirrors_and_preserves_repository_details() {
        let original = concat!(
            "# https://dl-cdn.alpinelinux.org/alpine/v3.22/community\n",
            "  http://dl-cdn.alpinelinux.org/alpine/v3.22/main # stable\n",
            "https://dl-2.alpinelinux.org/alpine/v3.22/community\n",
            "@testing https://mirrors.ustc.edu.cn/alpine/edge/testing\n",
            "https://private.example/alpine/v3.22/main\n",
            "/mnt/packages\n",
        );
        for mirror in MirrorName::all() {
            let (content, skipped) = rewrite(original, mirror);
            let host = mirror_host(mirror).unwrap_or(OFFICIAL_HOST);
            assert!(content.contains(&format!("  https://{host}/alpine/v3.22/main # stable\n")));
            assert!(content.contains(&format!("@testing https://{host}/alpine/edge/testing\n")));
            assert!(content.contains(&format!("https://{host}/alpine/v3.22/community\n")));
            assert!(content.starts_with("# https://dl-cdn.alpinelinux.org"));
            assert!(
                content.ends_with("https://private.example/alpine/v3.22/main\n/mnt/packages\n")
            );
            assert_eq!(skipped, 2);
            assert_eq!(rewrite(&content, mirror), (content, 2));
        }
    }

    #[test]
    fn preserves_crlf_and_missing_final_newline() {
        let original = "\t@edge  https://dl-cdn.alpinelinux.org/alpine/edge/main\r\nhttps://dl-cdn.alpinelinux.org/alpine/v3.22/community";
        assert_eq!(rewrite(original, MirrorName::Ustc), (
            "\t@edge  https://mirrors.ustc.edu.cn/alpine/edge/main\r\nhttps://mirrors.ustc.edu.cn/alpine/v3.22/community".into(),
            0,
        ));
    }

    #[test]
    fn does_not_rewrite_unrelated_or_lookalike_urls() {
        let original = concat!(
            "https://mirrors.ustc.edu.cn/other/v3.22/main\n",
            "https://dl-cdn.alpinelinux.org.evil.example/alpine/v3.22/main\n",
            "https://user@dl-cdn.alpinelinux.org/alpine/v3.22/main\n",
            "https://dl-cdn.alpinelinux.org:8443/alpine/v3.22/main\n",
        );
        assert_eq!(rewrite(original, MirrorName::Tuna), (original.into(), 4));
    }
}

#[cfg(all(test, feature = "test-support"))]
mod apply_tests {
    use super::*;
    use crate::mirror::{MirrorPaths, test_support::TestPathsGuard};

    struct Fixture {
        root: std::path::PathBuf,
        _guard: TestPathsGuard,
    }

    impl Fixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lkit-apk-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("etc/apk")).unwrap();
            let guard = TestPathsGuard::set(MirrorPaths {
                apk_repositories: root.join("etc/apk/repositories"),
                backup_root: root.join("backups"),
                restore_root: root.clone(),
                os_release: root.join("etc/os-release"),
                allow_non_root: true,
                skip_refresh: true,
                ..MirrorPaths::production()
            });
            Self {
                root,
                _guard: guard,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn apply_show_restore_and_noop_preserve_backup_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _fixture = Fixture::new();
        let original = "https://dl-cdn.alpinelinux.org/alpine/v3.22/main\n";
        let path = &paths().apk_repositories;
        fs::write(path, original).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o640)).unwrap();
        let report = ApkBackend.apply(MirrorName::Ustc, false, true).unwrap();
        assert_eq!(report.changed_files, 1);
        assert!(
            ApkBackend
                .show()
                .unwrap()
                .contains("mirrors.ustc.edu.cn/alpine/v3.22/main")
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            ApkBackend
                .apply(MirrorName::Ustc, false, true)
                .unwrap()
                .changed_files,
            0
        );
        assert_eq!(
            fs::read_to_string(report.backup_path.unwrap().join("etc/apk/repositories")).unwrap(),
            original
        );
        ApkBackend.restore().unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert!(!backup_dir(Family::Alpine).exists());
        assert!(ApkBackend.restore().is_err());
    }

    #[test]
    fn missing_and_custom_sources_do_not_create_backups() {
        let _fixture = Fixture::new();
        assert!(ApkBackend.show().is_err());
        assert!(ApkBackend.apply(MirrorName::Ustc, false, true).is_err());
        let original = "https://private.example/packages\n";
        fs::write(&paths().apk_repositories, original).unwrap();
        let report = ApkBackend.apply(MirrorName::Ustc, false, true).unwrap();
        assert_eq!(report.changed_files, 0);
        assert_eq!(report.skipped_repositories, 1);
        assert!(!backup_dir(Family::Alpine).exists());
        assert_eq!(
            fs::read_to_string(&paths().apk_repositories).unwrap(),
            original
        );
    }

    #[test]
    fn write_failure_rolls_back() {
        let _fixture = Fixture::new();
        let path = &paths().apk_repositories;
        let original = "https://dl-cdn.alpinelinux.org/alpine/edge/main\n";
        fs::write(path, original).unwrap();
        let obstruction = path
            .parent()
            .unwrap()
            .join(format!(".lkit-mirror-{}.tmp", std::process::id()));
        fs::write(obstruction, "occupied").unwrap();
        assert!(ApkBackend.apply(MirrorName::Ustc, false, true).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert!(!backup_dir(Family::Alpine).exists());
    }
}
