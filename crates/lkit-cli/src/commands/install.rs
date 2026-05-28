//! `lkit install` — skeleton, not yet implemented.

use std::collections::HashMap;

use crate::messages::CliMessages;

/// Run the install command (skeleton — not yet implemented).
pub(crate) async fn run() -> anyhow::Result<()> {
    let mut params = HashMap::new();
    params.insert("milestone", "M2");
    eprintln!("{}", CliMessages::format("not_implemented", &params));
    Ok(())
}
