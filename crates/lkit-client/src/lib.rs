//! lkit-client: external I/O implementations — API client, systemd, log reader.

mod client;
mod log_reader;
mod systemd;

pub use client::LandscapeClient;
pub use log_reader::FileLogReader;
pub use systemd::SystemdManager;
