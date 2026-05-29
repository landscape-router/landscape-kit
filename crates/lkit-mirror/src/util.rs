//! Shared utilities for mirror operations.

use std::path::PathBuf;

/// Normalize a path by resolving `.` and `..` without requiring the path to exist.
pub fn normalize_path(path: &std::path::Path) -> PathBuf {
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
