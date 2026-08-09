use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostNetError {
    #[error("cannot read or write {path}: {source}")]
    UnreadableFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}:{line}: unsupported syntax: {reason}")]
    UnsupportedSyntax {
        path: PathBuf,
        line: usize,
        reason: String,
    },
    #[error("{path}: interface {iface} uses unsupported method {method}")]
    UnsupportedMethod {
        path: PathBuf,
        iface: String,
        method: String,
    },
    #[error("{path}: cannot expand source pattern {pattern}: {source}")]
    SourceExpansionFailed {
        path: PathBuf,
        pattern: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot atomically write {path}: {source}")]
    AtomicWriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ifupdown dry-run validation failed (exit {exit:?}): {stderr}")]
    ValidationFailed { exit: Option<i32>, stderr: String },
    #[error("invalid manifest at {path}: {reason}")]
    InvalidManifest { path: PathBuf, reason: String },
    #[error("unsafe path {path}: {reason}")]
    PathSafety { path: PathBuf, reason: String },
    #[error("file changed while preparing host network edit: {path}")]
    ConcurrentModification { path: PathBuf },
    #[error("host network operation failed: {operation}; restore also failed: {recovery}")]
    RecoveryFailed {
        operation: Box<HostNetError>,
        recovery: Box<HostNetError>,
    },
}
