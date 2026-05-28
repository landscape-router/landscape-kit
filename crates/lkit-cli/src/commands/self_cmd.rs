//! `lkit self` command handler.

use std::collections::HashMap;

use crate::cli::{SelfAction, SelfArgs};
use crate::messages::CliMessages;

pub async fn run(args: SelfArgs) -> anyhow::Result<()> {
    match args.action {
        SelfAction::Version => {
            let mut params = HashMap::new();
            params.insert("version", env!("CARGO_PKG_VERSION"));
            eprintln!("{}", CliMessages::format("self.version", &params));
        }
        SelfAction::UpgradeCheck => {
            let mut params = HashMap::new();
            params.insert("milestone", "V2");
            eprintln!("{}", CliMessages::format("not_implemented", &params));
        }
    }
    Ok(())
}
