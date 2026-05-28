//! Mirror target abstraction — trait + implementations.

pub mod local;
pub mod s3;

use async_trait::async_trait;

use crate::error::MirrorError;

/// Abstraction over mirror storage targets (local filesystem, S3, etc.).
#[async_trait]
pub trait MirrorTarget: Send + Sync {
    /// Upload data to the given key.
    async fn upload(&self, key: &str, data: &[u8]) -> Result<(), MirrorError>;
    /// Check if a key exists.
    async fn exists(&self, key: &str) -> Result<bool, MirrorError>;
    /// Read the content of a key.
    async fn read(&self, key: &str) -> Result<Vec<u8>, MirrorError>;
    /// List keys under a prefix.
    async fn list(&self, prefix: &str) -> Result<Vec<String>, MirrorError>;
    /// Delete a key.
    async fn delete(&self, key: &str) -> Result<(), MirrorError>;
}
