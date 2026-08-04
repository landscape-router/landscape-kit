use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode, ServiceManagerArg};

#[derive(Args)]
pub struct Install {
    /// Target version: `<version>` or `latest`
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    /// Repository source: bare flag uses the default HTTP mirror, a value uses the given protocol v1 HTTP repository
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    /// Full install root directory
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    /// First-install admin username, defaults to `admin`
    #[arg(long, value_name = "NAME")]
    pub admin_user: Option<String>,
    /// First-install password read from a restricted file
    #[arg(long, value_name = "PATH")]
    pub password_file: Option<PathBuf>,
    /// Password captured by the interactive console. Never populated by CLI parsing.
    #[arg(skip)]
    pub(crate) interactive_password: Option<String>,
    /// Service manager: `systemd` or `none`
    #[arg(long, value_enum)]
    pub service_manager: Option<ServiceManagerArg>,
    /// Prompt the user to manually clean the existing directory before a clean install
    #[arg(long)]
    pub force: bool,
    /// Interactively hand wired interfaces and host network services to Landscape
    #[arg(long)]
    pub takeover_network: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

impl std::fmt::Debug for Install {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Install")
            .field("version", &self.version)
            .field("repository", &self.repository)
            .field("install_dir", &self.install_dir)
            .field("admin_user", &self.admin_user)
            .field("password_file", &self.password_file)
            .field(
                "interactive_password",
                &self.interactive_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("service_manager", &self.service_manager)
            .field("force", &self.force)
            .field("takeover_network", &self.takeover_network)
            .finish_non_exhaustive()
    }
}

pub async fn run(args: &Install) -> ExitCode {
    super::manage::run_request(&InstallRequest {
        mode: RequestMode::Install,
        version: args.version.clone(),
        repository: args.repository.clone(),
        install_dir: args.install_dir.clone(),
        admin_user: args.admin_user.clone(),
        password_file: args.password_file.clone(),
        interactive_password: args.interactive_password.clone(),
        service_manager: args.service_manager,
        repair_static: false,
        repair_binary: false,
        allow_no_backup: false,
        accept_service_change: false,
        force: args.force,
        takeover_network: args.takeover_network,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<Install, clap::Error> {
        let command = <Install as Args>::augment_args(Command::new("install"));
        let matches = command.try_get_matches_from(args)?;
        Install::from_arg_matches(&matches)
    }

    #[test]
    fn parses_first_install_options() {
        let install = parse(&[
            "install",
            "--repository",
            "--version",
            "0.19.2",
            "--service-manager",
            "none",
        ])
        .unwrap();
        assert_eq!(install.repository, Some(None));
        assert_eq!(install.version.as_deref(), Some("0.19.2"));
        assert_eq!(install.service_manager, Some(ServiceManagerArg::None));
        assert!(!install.takeover_network);
    }

    #[test]
    fn parses_network_takeover_flag() {
        let install = parse(&["install", "--takeover-network"]).unwrap();
        assert!(install.takeover_network);
    }

    #[test]
    fn rejects_non_install_workflow_flags() {
        assert!(parse(&["install", "--repair-static"]).is_err());
        assert!(parse(&["install", "--accept-service-change"]).is_err());
    }
}
