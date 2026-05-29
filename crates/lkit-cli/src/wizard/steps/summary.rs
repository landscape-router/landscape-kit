//! Step 7: Summary confirmation.

use anyhow::Result;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{ContentArrangement, Table};
use dialoguer::Select;

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the installation summary and ask for confirmation.
pub fn render(collected: &CollectedConfig) -> Result<WizardAction> {
    let has_lan = !collected.lan_nics.is_empty();

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Disabled)
        .set_width(46);

    // Title row (header gets a heavy ═══ separator below automatically).
    table.set_header(vec!["安装摘要"]);

    // Section: 网络
    table.add_row(vec!["网络"]);

    let wan_desc = match &collected.wan_mode {
        Some(lkit_core::WanMode::Dhcp) => "DHCP".to_string(),
        Some(lkit_core::WanMode::Static { ipv4, mask, .. }) => {
            format!("静态 IP {ipv4}/{mask}")
        }
        Some(lkit_core::WanMode::Nothing) => "不配置".to_string(),
        None => "未选择".to_string(),
    };
    let wan_nic = collected.wan_nic.as_deref().unwrap_or("?");
    table.add_row(vec![format!("  WAN: {wan_nic} · {wan_desc}")]);

    if has_lan {
        let lan_nics = collected.lan_nics.join(" + ");
        table.add_row(vec![format!("  LAN: {lan_nics} → br_lan")]);
        if let (Some(gw), Some(mask)) = (collected.lan_gateway, collected.lan_mask) {
            table.add_row(vec![format!("  网关: {gw}/{mask}")]);
        }
        table.add_row(vec!["  DHCP: 自动生成"]);
    } else {
        table.add_row(vec!["  LAN: 无（单网卡模式）"]);
    }

    // Section: 服务
    table.add_row(vec!["服务"]);
    let port = collected.web_port.unwrap_or(6300);
    let user = collected.admin_user.as_deref().unwrap_or("root");
    table.add_row(vec![format!("  Web 端口: {port}")]);
    table.add_row(vec![format!("  管理员: {user}")]);

    // Section: 安装源
    table.add_row(vec!["安装源"]);
    let source_display = collected.source_name.as_deref().unwrap_or("自动探测");
    let version_display = collected.version.as_deref().unwrap_or("最新版");
    table.add_row(vec![format!("  来源: {source_display}")]);
    table.add_row(vec![format!("  版本: {version_display}")]);

    // Footer note
    if has_lan {
        table.add_row(vec!["NAT、防火墙等高级服务请安装后在 Web UI 配置"]);
    } else {
        table.add_row(vec!["LAN 和高级服务请安装后在 Web UI 配置"]);
    }

    // Indent the whole table by 2 spaces.
    for line in table.to_string().lines() {
        eprintln!("  {line}");
    }
    eprintln!();

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
        _ => anyhow::bail!("unexpected selection: {selection}"),
    }
}
