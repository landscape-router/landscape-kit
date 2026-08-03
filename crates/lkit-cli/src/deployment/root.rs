use std::path::{Path, PathBuf};

use super::plan::InstallError;

const DANGEROUS_ROOTS: [&str; 3] = ["/", "/root", "/root/.lkit"];
const MANAGED_DIRS: [&str; 8] = [
    "releases",
    "data",
    "state",
    "transactions",
    "backups",
    "run",
    "service",
    "logs",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstallRoot {
    /// 用户指定或默认得到的安装根目录。
    pub install_root: PathBuf,
    /// 解析后的真实路径。
    pub canonical: PathBuf,
}

pub(crate) fn normalize_install_root(path: &Path) -> Result<InstallRoot, InstallError> {
    if !path.is_absolute() {
        return Err(InstallError::InstallDirNotAbsolute);
    }
    let lexical = std::path::absolute(path).map_err(InstallError::Io)?;
    reject_dangerous_root(&lexical)?;
    let canonical = canonicalize_nearest_existing(&lexical)?;
    reject_dangerous_root(&canonical)?;
    verify_managed_dirs(&canonical)?;
    Ok(InstallRoot {
        install_root: path.to_path_buf(),
        canonical,
    })
}

fn reject_dangerous_root(path: &Path) -> Result<(), InstallError> {
    if DANGEROUS_ROOTS
        .iter()
        .any(|dangerous| path == Path::new(dangerous))
    {
        return Err(InstallError::DangerousDirectory(format!(
            "{} is a dangerous parent directory",
            path.display()
        )));
    }
    Ok(())
}

fn canonicalize_nearest_existing(path: &Path) -> Result<PathBuf, InstallError> {
    let mut existing = path;
    let mut suffix: Vec<PathBuf> = Vec::new();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(PathBuf::from(name));
                existing = parent;
            }
            _ => break,
        }
    }
    let mut canonical = std::fs::canonicalize(existing).map_err(InstallError::Io)?;
    for part in suffix.iter().rev() {
        canonical.push(part);
    }
    Ok(canonical)
}

fn verify_managed_dirs(canonical: &Path) -> Result<(), InstallError> {
    for dir in MANAGED_DIRS {
        let path = canonical.join(dir);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(InstallError::Io(error)),
        };
        if metadata.file_type().is_symlink() {
            let target = match std::fs::canonicalize(&path) {
                Ok(target) => target,
                Err(_) => {
                    return Err(InstallError::DangerousDirectory(format!(
                        "{} is a broken symbolic link",
                        path.display()
                    )));
                }
            };
            if !target.starts_with(canonical) {
                return Err(InstallError::DangerousDirectory(format!(
                    "{} points outside the install root",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-root-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn normalizes_existing_directory() {
        let temp = temp_root("existing");
        let root = normalize_install_root(&temp).unwrap();
        assert_eq!(root.canonical, std::fs::canonicalize(&temp).unwrap());
        assert_eq!(root.install_root, temp);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn normalizes_missing_nested_directory() {
        let temp = temp_root("nested");
        let target = temp.join("a").join("b").join("c");
        let root = normalize_install_root(&target).unwrap();
        assert_eq!(
            root.canonical,
            std::fs::canonicalize(&temp).unwrap().join("a/b/c")
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(matches!(
            normalize_install_root(Path::new("relative/dir")),
            Err(InstallError::InstallDirNotAbsolute)
        ));
    }

    #[test]
    fn rejects_dangerous_roots() {
        for dangerous in ["/", "/root", "/root/.lkit"] {
            assert!(matches!(
                normalize_install_root(Path::new(dangerous)),
                Err(InstallError::DangerousDirectory(_))
            ));
        }
        assert!(matches!(
            normalize_install_root(Path::new("/root/..")),
            Err(InstallError::DangerousDirectory(_))
        ));
    }

    #[test]
    fn rejects_managed_dir_escaping_the_root() {
        let temp = temp_root("managed");
        let outside = temp.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(temp.join("root")).unwrap();
        let canonical_root = std::fs::canonicalize(temp.join("root")).unwrap();
        std::os::unix::fs::symlink(&outside, canonical_root.join("releases")).unwrap();
        assert!(matches!(
            normalize_install_root(&canonical_root),
            Err(InstallError::DangerousDirectory(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn accepts_managed_dir_symlink_inside_the_root() {
        let temp = temp_root("managed-inside");
        let root = temp.join("root");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::os::unix::fs::symlink(root.join("data"), root.join("logs")).unwrap();
        assert!(normalize_install_root(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_broken_managed_dir_symlink() {
        let temp = temp_root("managed-broken");
        let root = temp.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(root.join("missing-target"), root.join("state")).unwrap();
        assert!(matches!(
            normalize_install_root(&root),
            Err(InstallError::DangerousDirectory(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }
}
