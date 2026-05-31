//! Path discovery for Landscape and manager working directories.

use std::path::PathBuf;

/// Discovered paths within the Landscape HOME directory.
#[derive(Debug, Clone)]
pub struct LandscapePaths {
    /// Root of the Landscape installation.
    pub home: PathBuf,
    /// Landscape runtime configuration file.
    pub landscape_config: PathBuf,
    /// Landscape SQLite database.
    pub db_file: PathBuf,
    /// Initialization lock file — presence skips re-initialization.
    pub init_lock: PathBuf,
    /// Frontend static assets directory.
    pub static_dir: PathBuf,
    /// Landscape logs directory.
    pub logs_dir: PathBuf,
    /// Landscape API JWT token file.
    pub api_token: PathBuf,
}

impl LandscapePaths {
    /// Derive all Landscape paths from the HOME directory.
    pub fn new(home: PathBuf) -> Self {
        Self {
            landscape_config: home.join("landscape.toml"),
            db_file: home.join("landscape_db.sqlite"),
            init_lock: home.join("landscape_init.lock"),
            static_dir: home.join("static"),
            logs_dir: home.join("logs"),
            api_token: home.join("landscape_api_token"),
            home,
        }
    }
}

/// Paths within the manager's own working directory (`~/.landscape-kit/`).
#[derive(Debug)]
pub struct ManagerPaths {
    /// Root of the manager working directory.
    pub home: PathBuf,
    /// Runtime state (e.g. pidfile).
    pub runtime_dir: PathBuf,
    /// Temporary staging area.
    pub tmp_dir: PathBuf,
    /// Backup repository.
    pub backup_dir: PathBuf,
    /// Manager configuration.
    pub config_dir: PathBuf,
}

impl ManagerPaths {
    /// Derive all manager paths from the HOME directory.
    pub fn new(home: PathBuf) -> Self {
        Self {
            runtime_dir: home.join("runtime"),
            tmp_dir: home.join("tmp"),
            backup_dir: home.join("backup"),
            config_dir: home.join("config"),
            home,
        }
    }
}
