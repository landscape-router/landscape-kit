#[cfg(feature = "test-support")]
#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct Network {
    #[command(subcommand)]
    pub action: NetworkAction,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true, global = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    /// Validate the Landscape service startup recovery barrier
    #[command(hide = true)]
    StartupCheck,
    /// Show pending network changes or the committed network takeover
    Status,
    /// Confirm connectivity after installation, reinitialization or restore
    Confirm,
    /// Roll back unconfirmed installation, reinitialization or restore
    Rollback {
        #[arg(long, hide = true)]
        automatic: bool,
    },
}

pub async fn run(args: &Network) -> ExitCode {
    crate::network::takeover::run_command(args).await
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<Network, clap::Error> {
        let command = <Network as Args>::augment_args(Command::new("network"));
        let matches = command.try_get_matches_from(args)?;
        Network::from_arg_matches(&matches)
    }

    #[test]
    fn parses_confirmation_and_rejects_removed_install_dir() {
        let network = parse(&["network", "confirm"]).unwrap();
        assert!(matches!(network.action, NetworkAction::Confirm));
        assert!(parse(&["network", "confirm", "--install-dir", "/opt/landscape"]).is_err());
    }

    #[test]
    fn automatic_rollback_is_available_to_recovery_unit() {
        let network = parse(&["network", "rollback", "--automatic"]).unwrap();
        assert!(matches!(
            network.action,
            NetworkAction::Rollback { automatic: true }
        ));
    }
}
