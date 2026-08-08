use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct Network {
    #[command(subcommand)]
    pub action: NetworkAction,
    /// Full install root directory
    #[arg(long, value_name = "PATH", global = true)]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true, global = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    /// Show a pending or committed network takeover transaction
    Status,
    /// Confirm the network takeover
    Confirm,
    /// Restore the host network state saved before takeover
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
    fn parses_confirmation_and_install_root() {
        let network = parse(&["network", "confirm", "--install-dir", "/opt/landscape"]).unwrap();
        assert!(matches!(network.action, NetworkAction::Confirm));
        assert_eq!(network.install_dir, Some(PathBuf::from("/opt/landscape")));
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
