//! Source resolution — multi-source probing and selection.

pub mod build;
pub mod config_loader;
pub mod resolver;

pub use build::build_release_sources;
pub use config_loader::load_lkit_toml;
pub use resolver::{ProbeResult, SourceResolver};
