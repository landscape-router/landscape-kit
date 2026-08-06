use std::path::{Path, PathBuf};

use lkit_repository::parse_stable_version;
use semver::Version;

use super::repository::github::DEFAULT_REPOSITORY;
use super::repository::http::HttpRepository;
use super::repository::{ProviderKind, RepositoryError};
use super::root::InstallRoot;

pub(crate) const DEFAULT_INSTALL_ROOT: &str = "/root/.lkit/landscape";
pub(crate) const DEFAULT_HTTP_MIRROR: &str = "https://l1s3.whileaway.dev/landscape/";

const CREDENTIAL_DATA_PROBES: [&str; 3] = [
    "data/landscape_init.lock",
    "data/landscape.toml",
    "data/landscape_db.sqlite",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TargetVersion {
    Latest,
    Version(Version),
}

impl TargetVersion {
    pub(crate) fn parse(value: &str) -> Result<Self, InstallError> {
        if value == "latest" {
            return Ok(Self::Latest);
        }
        let canonical = value.strip_prefix('v').unwrap_or(value);
        let version =
            parse_stable_version(canonical).map_err(|error| InstallError::InvalidVersion {
                value: value.into(),
                reason: error.to_string(),
            })?;
        Ok(Self::Version(version))
    }
}

impl std::fmt::Display for TargetVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Latest => write!(f, "latest"),
            Self::Version(version) => write!(f, "{version}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderSpec {
    pub kind: ProviderKind,
    pub location: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RepositoryChoice {
    Github,
    Mirror,
    Http(String),
}

impl RepositoryChoice {
    pub(crate) fn resolve(self) -> Result<ProviderSpec, InstallError> {
        match self {
            Self::Github => Ok(ProviderSpec {
                kind: ProviderKind::Github,
                location: DEFAULT_REPOSITORY.into(),
            }),
            Self::Mirror => Ok(ProviderSpec {
                kind: ProviderKind::Http,
                location: DEFAULT_HTTP_MIRROR.into(),
            }),
            Self::Http(url) => {
                let repository = HttpRepository::new(&url)?;
                Ok(ProviderSpec {
                    kind: ProviderKind::Http,
                    location: repository.location().to_string(),
                })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatePresence {
    FirstInstall,
    Installed,
}

pub(crate) fn select_install_root(
    cli: Option<&Path>,
    env_value: Option<&str>,
) -> Result<PathBuf, InstallError> {
    let path = match cli {
        Some(path) => Some(path.to_path_buf()),
        None => env_value.map(PathBuf::from),
    }
    .unwrap_or_else(|| PathBuf::from(DEFAULT_INSTALL_ROOT));
    if !path.is_absolute() {
        return Err(InstallError::InstallDirNotAbsolute);
    }
    Ok(path)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct UsageFlags {
    pub admin_user: bool,
    pub password_file: bool,
    pub repair_static: bool,
    pub repair_binary: bool,
    pub accept_service_change: bool,
    pub force: bool,
}

impl UsageFlags {
    fn any_accept(self) -> bool {
        self.accept_service_change
    }

    fn any_repair(self) -> bool {
        self.repair_static || self.repair_binary
    }

    fn any_credential(self) -> bool {
        self.admin_user || self.password_file
    }
}

pub(crate) fn validate_applicability(
    state: StatePresence,
    install_root: &Path,
    flags: UsageFlags,
) -> Result<(), InstallError> {
    if flags.force && (flags.any_accept() || flags.any_repair()) {
        return Err(InstallError::ParameterUsage(
            "--force cannot be combined with --repair-static, --repair-binary, or any --accept-* flag"
                .into(),
        ));
    }
    match state {
        StatePresence::FirstInstall => {
            if flags.any_accept() || flags.any_repair() {
                return Err(InstallError::ParameterUsage(
                    "--repair-static, --repair-binary, and --accept-* flags are only allowed on an already installed environment"
                        .into(),
                ));
            }
            if flags.any_credential() {
                for probe in CREDENTIAL_DATA_PROBES {
                    if install_root.join(probe).exists() {
                        return Err(InstallError::ParameterUsage(
                            "existing installation data detected; --admin-user and --password-file are not allowed"
                                .into(),
                        ));
                    }
                }
            }
        }
        StatePresence::Installed => {
            if flags.any_credential() {
                return Err(InstallError::ParameterUsage(
                    "--admin-user and --password-file are only allowed on first install".into(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_admin_user(value: &str) -> Result<(), InstallError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(InstallError::InvalidAdminUser);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub target: TargetVersion,
    pub provider: ProviderSpec,
    pub root: InstallRoot,
    pub state: StatePresence,
}

pub(crate) fn build_plan(
    root: InstallRoot,
    target: TargetVersion,
    repository: RepositoryChoice,
    state: StatePresence,
) -> Result<Plan, InstallError> {
    Ok(Plan {
        target,
        provider: repository.resolve()?,
        root,
        state,
    })
}

#[derive(Debug)]
pub(crate) enum InstallError {
    InvalidVersion { value: String, reason: String },
    InstallDirNotAbsolute,
    InvalidAdminUser,
    ParameterUsage(String),
    UserRefused(String),
    Preflight(String),
    CorruptedState(String),
    CorruptedTransaction(String),
    BlockedByTransaction(String),
    ActivationDrift(String),
    DangerousDirectory(String),
    LockBusy,
    UnsupportedPlatform(String),
    NoStableVersion,
    ReleaseExists(String),
    InvalidPassword(String),
    InvalidPasswordFile(String),
    InvalidBackup(String),
    ExportFailed(String),
    ServiceNotRunning(String),
    NonInteractive(String),
    Systemd(String),
    HealthCheck(String),
    ProcessConflict(String),
    ResolvBackup(String),
    StateWrite(serde_json::Error),
    Repository(RepositoryError),
    Io(std::io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidVersion { value, reason } => write!(
                formatter,
                "{}",
                crate::tr!(
                    crate::keys::PLAN_INVALID_VERSION,
                    value = format!("{:?}", value),
                    reason = reason
                )
            ),
            Self::InstallDirNotAbsolute => {
                formatter.write_str(&crate::tr!(crate::keys::PLAN_INSTALL_DIR_NOT_ABSOLUTE))
            }
            Self::InvalidAdminUser => {
                formatter.write_str(&crate::tr!(crate::keys::PLAN_INVALID_ADMIN_USER))
            }
            Self::ParameterUsage(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_PARAMETER_USAGE_ERROR, reason = reason)
            ),
            Self::UserRefused(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_REFUSED, reason = reason)
            ),
            Self::Preflight(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_PREFLIGHT_CHECK_FAILED, reason = reason)
            ),
            Self::CorruptedState(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_INSTALL_STATE_CORRUPTED, reason = reason)
            ),
            Self::CorruptedTransaction(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_TRANSACTION_CORRUPTED, reason = reason)
            ),
            Self::BlockedByTransaction(reason) => write!(
                formatter,
                "{}",
                crate::tr!(
                    crate::keys::PLAN_BLOCKED_BY_UNFINISHED_TRANSACTION,
                    reason = reason
                )
            ),
            Self::ActivationDrift(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_ACTIVATION_DRIFT, reason = reason)
            ),
            Self::DangerousDirectory(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_DANGEROUS_DIRECTORY, reason = reason)
            ),
            Self::LockBusy => formatter.write_str(&crate::tr!(crate::keys::PLAN_LOCK_BUSY)),
            Self::UnsupportedPlatform(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_UNSUPPORTED_PLATFORM, reason = reason)
            ),
            Self::NoStableVersion => {
                formatter.write_str(&crate::tr!(crate::keys::PLAN_NO_STABLE_VERSION))
            }
            Self::ReleaseExists(version) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_RELEASE_EXISTS, version = version)
            ),
            Self::InvalidPassword(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_INVALID_PASSWORD, reason = reason)
            ),
            Self::InvalidPasswordFile(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_INVALID_PASSWORD_FILE, reason = reason)
            ),
            Self::InvalidBackup(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_INVALID_BACKUP, reason = reason)
            ),
            Self::ExportFailed(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_EXPORT_FAILED, reason = reason)
            ),
            Self::ServiceNotRunning(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_SERVICE_NOT_RUNNING, reason = reason)
            ),
            Self::NonInteractive(reason) => write!(
                formatter,
                "{}",
                crate::tr!(
                    crate::keys::PLAN_NON_INTERACTIVE_ENVIRONMENT,
                    reason = reason
                )
            ),
            Self::Systemd(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_SYSTEMD_OPERATION_FAILED, reason = reason)
            ),
            Self::HealthCheck(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_HEALTH_CHECK_FAILED, reason = reason)
            ),
            Self::ProcessConflict(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_CONFLICTING_PROCESS, reason = reason)
            ),
            Self::ResolvBackup(reason) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_HOST_STATE_BACKUP_FAILED, reason = reason)
            ),
            Self::StateWrite(error) => write!(
                formatter,
                "{}",
                crate::tr!(
                    crate::keys::PLAN_FAILED_TO_WRITE_INSTALL_STATE,
                    error = error
                )
            ),
            Self::Repository(error) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_REPOSITORY_SELECTION_FAILED, error = error)
            ),
            Self::Io(error) => write!(
                formatter,
                "{}",
                crate::tr!(crate::keys::PLAN_IO_ERROR, error = error)
            ),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<serde_json::Error> for InstallError {
    fn from(error: serde_json::Error) -> Self {
        Self::StateWrite(error)
    }
}

impl From<RepositoryError> for InstallError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<std::io::Error> for InstallError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_versions() {
        assert_eq!(
            TargetVersion::parse("latest").unwrap(),
            TargetVersion::Latest
        );
        assert_eq!(
            TargetVersion::parse("0.19.2").unwrap(),
            TargetVersion::Version(Version::new(0, 19, 2))
        );
        assert_eq!(
            TargetVersion::parse("v0.19.2").unwrap(),
            TargetVersion::Version(Version::new(0, 19, 2))
        );
        assert!(TargetVersion::parse("0.19").is_err());
        assert!(TargetVersion::parse("v0.19").is_err());
        assert!(TargetVersion::parse("0.20.0-rc.1").is_err());
        assert!(TargetVersion::parse("v0.20.0-rc.1").is_err());
        assert!(TargetVersion::parse("1.2.3+build.1").is_err());
        assert!(TargetVersion::parse("release-0.19.2").is_err());
        assert!(TargetVersion::parse("").is_err());
    }

    #[test]
    fn resolves_providers() {
        assert_eq!(
            RepositoryChoice::Github.resolve().unwrap(),
            ProviderSpec {
                kind: ProviderKind::Github,
                location: "ThisSeanZhang/landscape".into(),
            }
        );
        assert_eq!(
            RepositoryChoice::Mirror.resolve().unwrap(),
            ProviderSpec {
                kind: ProviderKind::Http,
                location: "https://l1s3.whileaway.dev/landscape/".into(),
            }
        );
        assert_eq!(
            RepositoryChoice::Http("https://example.com/mirror".into())
                .resolve()
                .unwrap(),
            ProviderSpec {
                kind: ProviderKind::Http,
                location: "https://example.com/mirror/".into(),
            }
        );
        assert!(matches!(
            RepositoryChoice::Http("http://example.com/mirror".into()).resolve(),
            Err(InstallError::Repository(_))
        ));
        assert!(matches!(
            RepositoryChoice::Http("https://example.com/mirror?x=1".into()).resolve(),
            Err(InstallError::Repository(_))
        ));
        assert!(matches!(
            RepositoryChoice::Http("https://user:pass@example.com/".into()).resolve(),
            Err(InstallError::Repository(_))
        ));
    }

    #[test]
    fn selects_install_root() {
        assert_eq!(
            select_install_root(None, None).unwrap(),
            PathBuf::from("/root/.lkit/landscape")
        );
        assert_eq!(
            select_install_root(Some(Path::new("/srv/landscape")), None).unwrap(),
            PathBuf::from("/srv/landscape")
        );
        assert_eq!(
            select_install_root(None, Some("/env/landscape")).unwrap(),
            PathBuf::from("/env/landscape")
        );
        assert_eq!(
            select_install_root(Some(Path::new("/srv/landscape")), Some("/env/landscape")).unwrap(),
            PathBuf::from("/srv/landscape")
        );
        assert!(select_install_root(Some(Path::new("relative/dir")), None).is_err());
        assert!(select_install_root(None, Some("relative/dir")).is_err());
    }

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-plan-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn validates_first_install_usage() {
        let root = temp_root("usage-first");
        let repair = UsageFlags {
            repair_static: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::FirstInstall, &root, repair).is_err());
        let accept = UsageFlags {
            accept_service_change: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::FirstInstall, &root, accept).is_err());
        assert!(
            validate_applicability(StatePresence::FirstInstall, &root, UsageFlags::default())
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_credentials_on_installed_environment() {
        let root = temp_root("usage-installed");
        let admin = UsageFlags {
            admin_user: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::Installed, &root, admin).is_err());
        let password = UsageFlags {
            password_file: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::Installed, &root, password).is_err());
        assert!(
            validate_applicability(StatePresence::Installed, &root, UsageFlags::default()).is_ok()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_credentials_when_installation_data_exists() {
        let root = temp_root("usage-data");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/landscape_init.lock"), b"").unwrap();
        let flags = UsageFlags {
            admin_user: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::FirstInstall, &root, flags).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_force_with_repair_or_accept() {
        let flags = UsageFlags {
            force: true,
            repair_binary: true,
            ..Default::default()
        };
        assert!(
            validate_applicability(StatePresence::Installed, Path::new("/tmp"), flags).is_err()
        );
        let flags = UsageFlags {
            force: true,
            accept_service_change: true,
            ..Default::default()
        };
        assert!(
            validate_applicability(StatePresence::Installed, Path::new("/tmp"), flags).is_err()
        );
        let flags = UsageFlags {
            force: true,
            ..Default::default()
        };
        assert!(validate_applicability(StatePresence::Installed, Path::new("/tmp"), flags).is_ok());
    }

    #[test]
    fn validates_admin_user() {
        assert!(validate_admin_user("admin").is_ok());
        assert!(validate_admin_user("router-1").is_ok());
        assert!(validate_admin_user("").is_err());
        assert!(validate_admin_user("a\nb").is_err());
        assert!(validate_admin_user("a\u{7f}b").is_err());
    }
}
