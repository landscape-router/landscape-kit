//! Step 6: Install source and version selection.

use anyhow::Result;
use dialoguer::Input;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the source and version selection step.
///
/// V1: simplified — accepts source name and version tag as text input.
/// Full source resolution (SourceResolver, latency probing) is deferred to later milestones.
pub fn render(collected: &mut CollectedConfig) -> Result<WizardAction> {
    let source: String = Input::new()
        .with_prompt("安装源名称（留空自动探测）")
        .allow_empty(true)
        .default(collected.source_name.clone().unwrap_or_default())
        .interact()?;

    let version: String = Input::new()
        .with_prompt("版本号（留空使用最新版）")
        .allow_empty(true)
        .default(collected.version.clone().unwrap_or_default())
        .interact()?;

    collected.source_name = if source.is_empty() {
        None
    } else {
        Some(source)
    };
    collected.version = if version.is_empty() {
        None
    } else {
        Some(version)
    };

    Ok(WizardAction::Next)
}
