//! Step 3: WAN IP configuration mode.

use std::net::Ipv4Addr;

use anyhow::Result;
use dialoguer::{Input, Select};

use lkit_core::WanMode;

use crate::wizard::{CollectedConfig, WizardAction};

const MODE_OPTIONS: &[&str] = &["DHCP（自动获取）", "静态 IP", "不配置接入方式"];

/// Render the WAN configuration step.
///
/// Presents IP mode selection, then collects static IP details if needed.
pub fn render(collected: &mut CollectedConfig) -> Result<WizardAction> {
    let default = match &collected.wan_mode {
        Some(WanMode::Dhcp) => 0,
        Some(WanMode::Static { .. }) => 1,
        Some(WanMode::Nothing) => 2,
        None => 0,
    };

    let selection = Select::new()
        .with_prompt("WAN 接入方式")
        .items(MODE_OPTIONS)
        .default(default)
        .interact()?;

    match selection {
        0 => {
            collected.wan_mode = Some(WanMode::Dhcp);
        }
        1 => {
            let (ipv4, mask, gateway) = collect_static_ip()?;
            collected.wan_mode = Some(WanMode::Static {
                ipv4,
                mask,
                gateway,
            });
        }
        2 => {
            collected.wan_mode = Some(WanMode::Nothing);
        }
        _ => unreachable!(),
    }

    Ok(WizardAction::Next)
}

/// Collect static IP address, subnet mask, and gateway from user input.
fn collect_static_ip() -> Result<(Ipv4Addr, u8, Ipv4Addr)> {
    let ipv4_str: String = Input::new()
        .with_prompt("IP 地址")
        .validate_with(|input: &String| -> Result<(), String> {
            input
                .parse::<Ipv4Addr>()
                .map(|_| ())
                .map_err(|_| "无效的 IPv4 地址".to_string())
        })
        .interact()?;

    let mask: u8 = Input::new()
        .with_prompt("子网掩码位数（如 24）")
        .default(24u8)
        .validate_with(|input: &u8| -> Result<(), String> {
            if *input >= 8 && *input <= 30 {
                Ok(())
            } else {
                Err("掩码范围 8~30".to_string())
            }
        })
        .interact()?;

    let gw_str: String = Input::new()
        .with_prompt("网关地址")
        .validate_with(|input: &String| -> Result<(), String> {
            input
                .parse::<Ipv4Addr>()
                .map(|_| ())
                .map_err(|_| "无效的 IPv4 地址".to_string())
        })
        .interact()?;

    Ok((ipv4_str.parse()?, mask, gw_str.parse()?))
}
