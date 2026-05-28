//! `lkit config` — skeleton, not yet implemented.

use std::collections::HashMap;

use crate::messages::CliMessages;

pub async fn run() -> anyhow::Result<()> {
    let mut params = HashMap::new();
    params.insert("milestone", "M3");
    eprintln!("{}", CliMessages::format("not_implemented", &params));
    Ok(())
}
