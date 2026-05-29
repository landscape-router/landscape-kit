//! Wizard step implementations — interactive UI for each configuration step.

pub mod lan_config;
pub mod lan_select;
pub mod landscape_svc;
pub mod source;
pub mod summary;
pub mod wan_config;
pub mod wan_select;

use anyhow::Result;

use crate::wizard::nic_scan::NicInfo;
use crate::wizard::{CollectedConfig, StepKind, WizardAction};

/// Dispatch rendering to the appropriate step handler.
pub fn render_step(
    kind: StepKind,
    collected: &mut CollectedConfig,
    nics: &[NicInfo],
) -> Result<WizardAction> {
    match kind {
        StepKind::WanSelect => wan_select::render(collected, nics),
        StepKind::LanSelect => lan_select::render(collected, nics),
        StepKind::WanConfig => wan_config::render(collected),
        StepKind::LanConfig => lan_config::render(collected),
        StepKind::LandscapeService => landscape_svc::render(collected),
        StepKind::Source => source::render(collected),
        StepKind::Summary => summary::render(collected),
    }
}
