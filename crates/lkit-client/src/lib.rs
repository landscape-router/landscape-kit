//! lkit-client: external I/O implementations — API client, systemd, log reader.

mod client;
mod systemd;

pub use client::LandscapeClient;
pub use systemd::SystemdManager;
