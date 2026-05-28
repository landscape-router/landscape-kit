//! Release source implementations.

pub mod github;
pub mod http_mirror;
pub mod local;

pub use github::GithubSource;
pub use http_mirror::HttpMirrorSource;
pub use local::LocalSource;
