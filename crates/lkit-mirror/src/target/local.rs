//! Local filesystem mirror target.

use std::path::PathBuf;

use async_trait::async_trait;

use super::MirrorTarget;
use crate::error::MirrorError;

/// Mirror target backed by a local directory.
pub struct LocalTarget {
    root: PathBuf,
}

impl LocalTarget {
    /// Create a new local target rooted at the given path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn full_path(&self, key: &str) -> PathBuf {
        let sanitized = key.trim_start_matches('/');
        let path = self.root.join(sanitized);
        let normalized = normalize_path(&path);
        if !normalized.starts_with(&self.root) {
            return self.root.join("__invalid__");
        }
        path
    }
}

#[async_trait]
impl MirrorTarget for LocalTarget {
    async fn upload(&self, key: &str, data: &[u8]) -> Result<(), MirrorError> {
        let path = self.full_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, data).await?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, MirrorError> {
        let path = self.full_path(key);
        match tokio::fs::metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(MirrorError::Io(e)),
        }
    }

    async fn read(&self, key: &str) -> Result<Vec<u8>, MirrorError> {
        let path = self.full_path(key);
        Ok(tokio::fs::read(&path).await?)
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, MirrorError> {
        let dir = self.full_path(prefix);
        let mut keys = Vec::new();

        match tokio::fs::metadata(&dir).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(keys),
            Err(e) => return Err(MirrorError::Io(e)),
        }

        collect_files_recursive(&dir, &self.root, &mut keys).await?;
        Ok(keys)
    }

    async fn delete(&self, key: &str) -> Result<(), MirrorError> {
        let path = self.full_path(key);
        match tokio::fs::metadata(&path).await {
            Ok(_) => tokio::fs::remove_file(&path).await?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(MirrorError::Io(e)),
        }
        Ok(())
    }
}

/// Recursively collect file paths relative to root.
fn collect_files_recursive<'a>(
    dir: &'a std::path::Path,
    root: &'a std::path::Path,
    keys: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), MirrorError>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursive(&path, root, keys).await?;
            } else {
                let relative = match path.strip_prefix(root) {
                    Ok(r) => r.to_string_lossy().into_owned(),
                    Err(_) => path.to_string_lossy().into_owned(),
                };
                keys.push(relative);
            }
        }
        Ok(())
    })
}

/// Normalize a path by resolving `.` and `..` without requiring the path to exist.
fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upload_and_read() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        target.upload("landscape/v1.0/test.txt", b"hello").await?;
        let data = target.read("landscape/v1.0/test.txt").await?;
        assert_eq!(data, b"hello");
        Ok(())
    }

    #[tokio::test]
    async fn exists_returns_true_for_existing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        target.upload("key.txt", b"data").await?;
        assert!(target.exists("key.txt").await?);
        assert!(!target.exists("missing.txt").await?);
        Ok(())
    }

    #[tokio::test]
    async fn list_returns_all_files() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        target.upload("a/1.txt", b"1").await?;
        target.upload("a/2.txt", b"2").await?;
        target.upload("b/3.txt", b"3").await?;

        let mut keys = target.list("a/").await?;
        keys.sort();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|k| k.ends_with("1.txt")));
        assert!(keys.iter().any(|k| k.ends_with("2.txt")));
        Ok(())
    }

    #[tokio::test]
    async fn list_empty_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        let keys = target.list("nonexistent/").await?;
        assert!(keys.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn delete_removes_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        target.upload("key.txt", b"data").await?;
        assert!(target.exists("key.txt").await?);

        target.delete("key.txt").await?;
        assert!(!target.exists("key.txt").await?);
        Ok(())
    }

    #[tokio::test]
    async fn delete_nonexistent_is_noop() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        target.delete("missing.txt").await?;
        Ok(())
    }

    #[tokio::test]
    async fn path_traversal_is_blocked() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        // Attempt to write outside root
        target.upload("../../etc/malicious.txt", b"pwned").await?;

        // The file should land in the safe __invalid__ path, not outside root
        assert!(!std::path::Path::new("/etc/malicious.txt").exists());
        Ok(())
    }
}
