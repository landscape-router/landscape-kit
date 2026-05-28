//! Interactive launcher — universal menu for all lkit features.

use std::collections::HashMap;
use std::io::IsTerminal;

use dialoguer::Select;
use lkit_app::AppState;

use crate::cli::{Commands, DiagnoseArgs, LogsArgs, ServiceAction, ServiceArgs, StatusArgs};
use crate::messages::{CliMessages, msg};

/// A single menu entry: label key and associated action.
struct MenuItem {
    label_key: &'static str,
    action: MenuAction,
}

#[derive(Clone, Copy)]
enum MenuAction {
    Dispatch(Commands),
    NotImplemented(&'static str),
    Exit,
}

/// All menu items in display order.
const MENU_ITEMS: &[MenuItem] = &[
    MenuItem {
        label_key: "menu.status",
        action: MenuAction::Dispatch(Commands::Status(StatusArgs { json: false })),
    },
    MenuItem {
        label_key: "menu.start",
        action: MenuAction::Dispatch(Commands::Service(ServiceArgs {
            action: ServiceAction::Start,
        })),
    },
    MenuItem {
        label_key: "menu.stop",
        action: MenuAction::Dispatch(Commands::Service(ServiceArgs {
            action: ServiceAction::Stop,
        })),
    },
    MenuItem {
        label_key: "menu.restart",
        action: MenuAction::Dispatch(Commands::Service(ServiceArgs {
            action: ServiceAction::Restart,
        })),
    },
    MenuItem {
        label_key: "menu.logs",
        action: MenuAction::Dispatch(Commands::Logs(LogsArgs { lines: 50 })),
    },
    MenuItem {
        label_key: "menu.diagnose",
        action: MenuAction::Dispatch(Commands::Diagnose(DiagnoseArgs { json: false })),
    },
    MenuItem {
        label_key: "menu.install",
        action: MenuAction::NotImplemented("M2"),
    },
    MenuItem {
        label_key: "menu.backup",
        action: MenuAction::NotImplemented("M3"),
    },
    MenuItem {
        label_key: "menu.restore",
        action: MenuAction::NotImplemented("M3"),
    },
    MenuItem {
        label_key: "menu.upgrade",
        action: MenuAction::NotImplemented("M3"),
    },
    MenuItem {
        label_key: "menu.rollback",
        action: MenuAction::NotImplemented("M3"),
    },
    MenuItem {
        label_key: "menu.config_export",
        action: MenuAction::NotImplemented("M3"),
    },
    MenuItem {
        label_key: "menu.exit",
        action: MenuAction::Exit,
    },
];

/// Run the interactive launcher. Loops until user selects "exit".
pub async fn run(state: &AppState) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        eprintln!("{}", msg("error.not_tty"));
        std::process::exit(1);
    }

    let labels: Vec<String> = MENU_ITEMS
        .iter()
        .map(|item| {
            let base = msg(item.label_key);
            match item.action {
                MenuAction::NotImplemented(_) => {
                    format!("{} {}", base, msg("menu.soon_suffix"))
                }
                _ => base,
            }
        })
        .collect();

    loop {
        let selection = Select::new()
            .with_prompt(msg("menu.title"))
            .items(&labels)
            .default(0)
            .interact()?;

        match MENU_ITEMS[selection].action {
            MenuAction::Dispatch(cmd) => {
                crate::commands::dispatch(cmd, state).await?;
            }
            MenuAction::NotImplemented(milestone) => {
                let mut params = HashMap::new();
                params.insert("milestone", milestone);
                eprintln!("{}", CliMessages::format("not_implemented", &params));
            }
            MenuAction::Exit => {
                std::process::exit(0);
            }
        }
    }
}
