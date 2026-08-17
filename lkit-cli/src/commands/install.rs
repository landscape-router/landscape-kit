use std::path::PathBuf;
use std::process::ExitCode;

use crate::network::config::NetworkPlan;
use clap::Args;

use super::manage::{InstallRequest, RequestMode};

#[derive(Args)]
pub struct Install {
    /// Target version: `<version>` or `latest`
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    /// Repository source: bare flag uses the default HTTP mirror, `github` uses the
    /// official GitHub repository, a value uses the given protocol v1 HTTP repository;
    /// omitted entirely, the config file or the official GitHub default applies
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
    /// Prompt the user to manually clean the existing directory before a clean install
    #[arg(long)]
    pub force: bool,
    /// Interactively hand wired interfaces and host network services to Landscape
    #[arg(long)]
    pub takeover_network: bool,
    /// Network plan captured by the full-screen console. Never populated by CLI parsing.
    #[arg(skip)]
    pub(crate) network_plan: Option<NetworkPlan>,
    /// Root-only network plan file created for an internal daemon worker.
    #[arg(long, value_name = "PATH", hide = true)]
    pub(crate) network_plan_file: Option<PathBuf>,
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
            .field("force", &self.force)
            .field("takeover_network", &self.takeover_network)
            .finish_non_exhaustive()
    }
}

pub async fn run(args: &Install) -> ExitCode {
    let network_plan = match (&args.network_plan, &args.network_plan_file) {
        (Some(plan), None) => Some(plan.clone()),
        (None, Some(path)) => match crate::daemon_worker::read_network_plan(path) {
            Ok(plan) => Some(plan),
            Err(error) => {
                eprintln!("install: {error}");
                return ExitCode::from(2);
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            eprintln!("install: internal network plans cannot be combined");
            return ExitCode::from(2);
        }
    };
    super::manage::run_request(&InstallRequest {
        mode: RequestMode::Install,
        version: args.version.clone(),
        repository: super::manage::repository_override(&args.repository),
        install_dir: args.install_dir.clone(),
        admin_user: args.admin_user.clone(),
        password_file: args.password_file.clone(),
        interactive_password: args.interactive_password.clone(),
        repair_static: false,
        repair_binary: false,
        allow_no_backup: false,
        accept_service_change: false,
        force: args.force,
        takeover_network: args.takeover_network,
        network_plan,
        console_confirmed: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;
    use crate::deployment::layout;

    fn parse(args: &[&str]) -> Result<Install, clap::Error> {
        let command = <Install as Args>::augment_args(Command::new("install"));
        let matches = command.try_get_matches_from(args)?;
        Install::from_arg_matches(&matches)
    }

    fn install_args() -> Install {
        Install {
            version: None,
            repository: None,
            install_dir: None,
            admin_user: None,
            password_file: None,
            interactive_password: None,
            force: false,
            takeover_network: false,
            network_plan: None,
            network_plan_file: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        }
    }

    /// 建立隔离 lkit 地盘,写入指定状态内容,返回 (守卫, 地盘)。
    fn territory_with_state(name: &str, bytes: &[u8]) -> (layout::TerritoryOverride, PathBuf) {
        let territory = std::env::temp_dir().join(format!(
            "lkit-install-command-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("state")).unwrap();
        let guard = layout::test_territory(&territory);
        std::fs::write(layout::territory_state_path(), bytes).unwrap();
        (guard, territory)
    }

    #[test]
    fn parses_first_install_options() {
        let install = parse(&["install", "--repository", "--version", "0.19.2"]).unwrap();
        assert_eq!(install.repository, Some(None));
        assert_eq!(install.version.as_deref(), Some("0.19.2"));
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

    #[tokio::test]
    async fn refuses_install_when_an_installation_state_exists() {
        let (_guard, _territory) = territory_with_state(
            "single-instance",
            b"{\"schema_version\":1,\"layout_version\":2,\"install_root\":\"/opt/landscape\",\"canonical_install_root\":\"/opt/landscape\",\"active_version\":\"0.19.2\",\"assets\":{\"webserver\":{\"architecture\":\"x86_64\",\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"size\":10},\"static_archive\":{\"sha256\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"size\":20}},\"initialization\":{\"status\":\"complete\",\"lock_present\":true,\"initialized_at\":\"2026-08-01T16:30:00Z\"},\"service\":{\"manager\":\"systemd\",\"registered\":true,\"enabled\":true,\"verified\":true,\"definition_path\":\"service/landscape-router.service\",\"definition_sha256\":\"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\"},\"last_transaction_id\":null,\"committed_at\":\"2026-08-01T16:30:00Z\"}",
        );
        let args = install_args();
        assert_eq!(run(&args).await, ExitCode::from(2));
    }

    #[tokio::test]
    async fn refuses_install_when_the_installation_state_is_corrupted() {
        let (_guard, _territory) = territory_with_state("corrupted-state", b"not json");
        let args = install_args();
        assert_eq!(run(&args).await, ExitCode::FAILURE);
    }
}
