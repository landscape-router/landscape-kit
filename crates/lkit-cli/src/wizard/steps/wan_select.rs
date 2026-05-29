//! Step 1: WAN NIC selection.

use anyhow::Result;
use dialoguer::Select;

use crate::wizard::nic_scan::NicInfo;
use crate::wizard::{CollectedConfig, WizardAction};

/// Render the WAN selection step.
///
/// With 2+ NICs, presents a FuzzySelect (falls back to Select).
/// With exactly 1 NIC, auto-selects.
/// With 0 NICs, returns an error.
pub fn render(collected: &mut CollectedConfig, nics: &[NicInfo]) -> Result<WizardAction> {
    if nics.is_empty() {
        anyhow::bail!("未检测到可用的物理网卡，无法继续安装");
    }

    if nics.len() == 1 {
        // Single NIC: auto-select as WAN
        collected.wan_nic = Some(nics[0].name.clone());
        eprintln!(
            "  检测到单网卡 {} ({})，将作为 WAN 使用，无 LAN",
            nics[0].name, nics[0].mac
        );
        return Ok(WizardAction::Next);
    }

    let items: Vec<String> = nics
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

    let default = collected
        .wan_nic
        .as_ref()
        .and_then(|name| nics.iter().position(|n| &n.name == name))
        .unwrap_or(0);

    let selection = Select::new()
        .with_prompt("选择 WAN 网卡")
        .items(&items)
        .default(default)
        .interact()?;

    collected.wan_nic = Some(nics[selection].name.clone());
    Ok(WizardAction::Next)
}
