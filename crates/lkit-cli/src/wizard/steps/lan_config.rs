//! Step 4: LAN gateway configuration.

use std::net::Ipv4Addr;

use anyhow::Result;
use dialoguer::Input;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the LAN gateway configuration step.
///
/// Collects gateway IP and subnet mask for the br_lan bridge.
pub fn render(collected: &mut CollectedConfig) -> Result<WizardAction> {
    let default_gw = collected.lan_gateway.unwrap_or(Ipv4Addr::new(192, 168, 5, 1));

    let gw_str: String = Input::new()
        .with_prompt("网关 IP")
        .default(default_gw.to_string())
        .validate_with(|input: &String| -> Result<(), String> {
            input.parse::<Ipv4Addr>().map(|_| ()).map_err(|_| "无效的 IPv4 地址".to_string())
        })
        .interact()?;

    let default_mask = collected.lan_mask.unwrap_or(24);
    let mask: u8 = Input::new()
        .with_prompt("子网掩码位数（如 24）")
        .default(default_mask)
        .validate_with(|input: &u8| -> Result<(), String> {
            if *input >= 8 && *input <= 29 { Ok(()) } else { Err("掩码范围 8~29".to_string()) }
        })
        .interact()?;

    collected.lan_gateway = Some(gw_str.parse()?);
    collected.lan_mask = Some(mask);

    Ok(WizardAction::Next)
}
