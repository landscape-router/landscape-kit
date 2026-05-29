//! Step 6: Version confirmation (auto-resolved from source).

use anyhow::Result;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the version confirmation step.
///
/// Version is pre-resolved by the install command via SourceResolver.
/// This step just displays it and lets the user proceed.
pub fn render(collected: &CollectedConfig) -> Result<WizardAction> {
    let ver = collected
        .version
        .as_deref()
        .unwrap_or("未确定");
    println!("  已确定版本: {ver}");
    Ok(WizardAction::Next)
}
