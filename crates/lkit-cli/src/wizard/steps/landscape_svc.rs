//! Step 5: Landscape service configuration (port, admin user, password).

use anyhow::Result;
use dialoguer::{Input, Password};

use crate::wizard::{CollectedConfig, WizardAction};

/// Render the Landscape service configuration step.
///
/// Collects web port, admin username, and admin password (with confirmation).
pub fn render(collected: &mut CollectedConfig) -> Result<WizardAction> {
    let port: u16 = Input::new()
        .with_prompt("Web 端口")
        .default(collected.web_port.unwrap_or(6300))
        .validate_with(|input: &u16| -> Result<(), String> {
            if *input > 0 { Ok(()) } else { Err("端口必须 > 0".to_string()) }
        })
        .interact()?;

    let user: String = Input::new()
        .with_prompt("管理员用户名")
        .default(
            collected
                .admin_user
                .clone()
                .unwrap_or_else(|| "root".to_string()),
        )
        .interact()?;

    let pass = Password::new()
        .with_prompt("管理员密码")
        .with_confirmation("确认密码", "密码不匹配")
        .interact()?;

    collected.web_port = Some(port);
    collected.admin_user = Some(user);
    collected.admin_pass = Some(pass);

    Ok(WizardAction::Next)
}
