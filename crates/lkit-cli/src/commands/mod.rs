mod check;

use clap::Subcommand;

pub use check::Check;

#[derive(Debug, Subcommand)]
pub enum Commands {
    Check(Check),
}
