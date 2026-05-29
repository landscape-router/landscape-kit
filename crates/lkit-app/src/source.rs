//! Source resolution — multi-source probing and selection.

pub mod build;
pub mod resolver;

pub use build::build_release_sources;
pub use resolver::{ProbeResult, SourceResolver};
