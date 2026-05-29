//! Step 6: Install source and version (read-only display).
//!
//! Source selection happens before the wizard in install.rs.
//! This step just displays the pre-resolved source and version.

use anyhow::Result;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the source info step (read-only).
///
/// Source name and version are pre-set by the install command.
/// This step displays them and lets the user proceed.
pub fn render(collected: &CollectedConfig) -> Result<WizardAction> {
    let source = collected.source_name.as_deref().unwrap_or("自动探测");
    let version = collected.version.as_deref().unwrap_or("未确定");
    eprintln!("  来源: {source}");
    eprintln!("  版本: {version}");
    Ok(WizardAction::Next)
}
