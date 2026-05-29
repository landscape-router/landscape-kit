//! Step 7: Summary confirmation.

use anyhow::Result;
use dialoguer::Select;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the installation summary and ask for confirmation.
pub fn render(collected: &CollectedConfig) -> Result<WizardAction> {
    let has_lan = !collected.lan_nics.is_empty();

    println!();
    println!("  ┌─ 安装摘要 ─────────────────────────────────┐");
    println!("  │ 网络                                       │");

    // WAN info
    let wan_desc = match &collected.wan_mode {
        Some(lkit_core::WanMode::Dhcp) => "DHCP".to_string(),
        Some(lkit_core::WanMode::Static { ipv4, mask, .. }) => {
            format!("静态 IP {ipv4}/{mask}")
        }
        Some(lkit_core::WanMode::Nothing) => "不配置".to_string(),
        None => "未选择".to_string(),
    };
    let wan_nic = collected.wan_nic.as_deref().unwrap_or("?");
    println!("  │   WAN: {wan_nic} · {wan_desc:<33}│");

    if has_lan {
        let lan_nics = collected.lan_nics.join(" + ");
        println!("  │   LAN: {lan_nics} → br_lan{:width$}│", "", width = 27_usize.saturating_sub(lan_nics.len()));
        if let (Some(gw), Some(mask)) = (collected.lan_gateway, collected.lan_mask) {
            let gw_str = format!("{gw}/{mask}");
            println!("  │   网关: {gw_str:<33}│");
        }
        println!("  │   DHCP: 自动生成{:width$}│", "", width = 26);
    } else {
        println!("  │   LAN: 无（单网卡模式）{:width$}│", "", width = 18);
    }

    println!("  │                                            │");
    println!("  │ 服务                                       │");
    let port = collected.web_port.unwrap_or(6300);
    let user = collected.admin_user.as_deref().unwrap_or("root");
    println!("  │   Web 端口: {port:<29}│");
    println!("  │   管理员: {user:<31}│");
    println!("  │                                            │");
    println!("  │ 安装源                                     │");
    let source_display = collected.source_name.as_deref().unwrap_or("自动探测");
    let version_display = collected.version.as_deref().unwrap_or("最新版");
    println!("  │   来源: {source_display:<33}│");
    println!("  │   版本: {version_display:<33}│");
    println!("  │                                            │");

    if has_lan {
        println!("  │ NAT、防火墙等高级服务请安装后在 Web UI 配置 │");
    } else {
        println!("  │ LAN 和高级服务请安装后在 Web UI 配置        │");
    }

    println!("  └────────────────────────────────────────────┘");
    println!();

    let options = &["确认安装", "返回修改", "退出"];
    let selection = Select::new()
        .with_prompt("操作")
        .items(options)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok(WizardAction::Next),
        1 => Ok(WizardAction::Back),
        2 => Ok(WizardAction::Quit),
        _ => unreachable!(),
    }
}
