//! Step 2: LAN NIC multi-selection.

use anyhow::Result;
use dialoguer::MultiSelect;

use crate::wizard::nic_scan::NicInfo;
use crate::wizard::{CollectedConfig, WizardAction};

/// Render the LAN NIC selection step.
///
/// Shows all non-WAN NICs as multi-select options. At least one must be selected.
pub fn render(collected: &mut CollectedConfig, nics: &[NicInfo]) -> Result<WizardAction> {
    let wan_nic = collected.wan_nic.as_deref().unwrap_or("");
    let candidates: Vec<&NicInfo> = nics.iter().filter(|n| n.name != wan_nic).collect();

    if candidates.is_empty() {
        anyhow::bail!("没有可选的 LAN 网卡（所有网卡已被选为 WAN）");
    }

    let items: Vec<String> = candidates
        .iter()
        .map(|n| {
            let ip_info = n
                .current_ip
                .as_deref()
                .map(|ip| format!(" {ip}"))
                .unwrap_or_default();
            format!("{} ({}){}", n.name, n.mac, ip_info)
        })
        .collect();

    // Pre-select previously selected LAN NICs
    let defaults: Vec<bool> = candidates
        .iter()
        .map(|n| collected.lan_nics.contains(&n.name))
        .collect();

    let selections = MultiSelect::new()
        .with_prompt("选择 LAN 网卡（空格选择，Enter 确认）")
        .items(&items)
        .defaults(&defaults)
        .interact()?;

    if selections.is_empty() {
        return Ok(WizardAction::Retry("至少选择一个 LAN 网卡".into()));
    }

    collected.lan_nics = selections
        .iter()
        .map(|&i| candidates[i].name.clone())
        .collect();
    Ok(WizardAction::Next)
}
