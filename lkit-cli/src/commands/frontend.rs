use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::deployment::config::{
    FrontendSource, RepositorySourceKind, load_frontend, save_frontend,
};
use crate::deployment::plan::InstallError;

#[derive(Debug, Args)]
pub struct FrontendAdd {
    /// Frontend source id (unique)
    pub id: String,
    /// Frontend source location: HTTP protocol v1 base URL or GitHub owner/repo
    pub location: String,
    /// Optional display name
    #[arg(long)]
    pub name: Option<String>,
    /// Activate this source immediately after adding it
    #[arg(long)]
    pub activate: bool,
}

#[derive(Debug, Args)]
pub struct FrontendSelect {
    /// Source id to activate, or `official` for the official frontend
    pub id: String,
}

#[derive(Debug, Args)]
pub struct FrontendRemove {
    /// Source id to remove
    pub id: String,
}

#[derive(Debug, Subcommand)]
pub enum FrontendAction {
    Add(FrontendAdd),
    Select(FrontendSelect),
    Remove(FrontendRemove),
    List,
    Status,
}

#[derive(Debug, Args)]
pub struct Frontend {
    #[command(subcommand)]
    pub action: FrontendAction,
}

pub fn run(args: &Frontend) -> ExitCode {
    let result = match &args.action {
        FrontendAction::Add(args) => add(args),
        FrontendAction::Select(args) => select(args),
        FrontendAction::Remove(args) => remove(args),
        FrontendAction::List => list(),
        FrontendAction::Status => status(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("frontend: {error}");
            match error {
                InstallError::ParameterUsage(_) => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

fn add(args: &FrontendAdd) -> Result<(), InstallError> {
    if args.id == crate::deployment::config::FRONTEND_OFFICIAL {
        return Err(InstallError::ParameterUsage(format!(
            "the id {:?} is reserved; choose another id",
            args.id
        )));
    }
    let (kind, normalized) = normalize_location(&args.location)?;
    let mut section = load_frontend()?.unwrap_or_default();
    if section.sources.iter().any(|source| source.id == args.id) {
        return Err(InstallError::ParameterUsage(format!(
            "a frontend source with id {:?} already exists",
            args.id
        )));
    }
    section.sources.push(FrontendSource {
        id: args.id.clone(),
        name: args.name.clone(),
        kind,
        location: normalized,
    });
    if args.activate {
        section.active = Some(args.id.clone());
    }
    save_frontend(&section)?;
    println!(
        "frontend: {} source {:?} registered",
        if args.activate { "activated" } else { "added" },
        args.id
    );
    if !args.activate {
        println!(
            "frontend: it takes effect after `lkit frontend select {}`, the next install/update/switch, or `lkit repair static`",
            args.id
        );
    } else {
        println!(
            "frontend: it takes effect on the next install/update/switch or `lkit repair static`"
        );
    }
    Ok(())
}

fn select(args: &FrontendSelect) -> Result<(), InstallError> {
    let mut section = load_frontend()?.unwrap_or_default();
    if args.id == crate::deployment::config::FRONTEND_OFFICIAL {
        section.active = None;
        save_frontend(&section)?;
        println!("frontend: switched to the official frontend");
        return Ok(());
    }
    if !section.sources.iter().any(|source| source.id == args.id) {
        return Err(InstallError::ParameterUsage(format!(
            "unknown frontend source {:?}; valid values are: official, {}",
            args.id,
            section
                .sources
                .iter()
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    section.active = Some(args.id.clone());
    save_frontend(&section)?;
    println!(
        "frontend: activated source {:?}; it takes effect on the next install/update/switch or `lkit repair static`",
        args.id
    );
    Ok(())
}

fn remove(args: &FrontendRemove) -> Result<(), InstallError> {
    let mut section = load_frontend()?.unwrap_or_default();
    let Some(index) = section
        .sources
        .iter()
        .position(|source| source.id == args.id)
    else {
        return Err(InstallError::ParameterUsage(format!(
            "unknown frontend source {:?}",
            args.id
        )));
    };
    if section.active.as_deref() == Some(args.id.as_str()) {
        return Err(InstallError::ParameterUsage(format!(
            "source {:?} is active; select another source or `official` before removing it",
            args.id
        )));
    }
    section.sources.remove(index);
    save_frontend(&section)?;
    println!("frontend: removed source {:?}", args.id);
    Ok(())
}

fn list() -> Result<(), InstallError> {
    let section = load_frontend()?.unwrap_or_default();
    let active = section
        .active
        .as_deref()
        .unwrap_or(crate::deployment::config::FRONTEND_OFFICIAL);
    println!("active: {active}");
    if section.sources.is_empty() {
        println!("frontend: no custom frontend sources registered");
        return Ok(());
    }
    for source in &section.sources {
        let marker = if source.id == active { "*" } else { " " };
        let name = source.name.as_deref().unwrap_or("-");
        println!(
            "{marker} {}  name={name}  kind={}  location={}",
            source.id,
            source.kind.key(),
            source.location
        );
    }
    Ok(())
}

fn status() -> Result<(), InstallError> {
    let section = load_frontend()?.unwrap_or_default();
    let active = section
        .active
        .as_deref()
        .unwrap_or(crate::deployment::config::FRONTEND_OFFICIAL);
    if active == crate::deployment::config::FRONTEND_OFFICIAL {
        println!("frontend: official pages are in use (no custom frontend source is active)");
    } else {
        let source = section
            .sources
            .iter()
            .find(|source| source.id == active)
            .expect("active id is validated by load_frontend");
        println!(
            "frontend: custom source {:?} (name={}) is active; kind={}, location={}",
            source.id,
            source.display_name(),
            source.kind.key(),
            source.location
        );
        println!(
            "frontend: changes take effect on the next install/update/switch or `lkit repair static`"
        );
    }
    Ok(())
}

/// 位置规范化:含 `://` 视为 HTTP protocol v1 base URL,否则按 GitHub `owner/repo`
/// 校验规范化。与 `[repository]` 的 CLI 裸参数语义一致。
fn normalize_location(location: &str) -> Result<(RepositorySourceKind, String), InstallError> {
    if location.contains("://") {
        let provider = crate::release::repository::provider_for(
            crate::release::repository::ProviderKind::Http,
            location,
        )
        .map_err(|error| {
            InstallError::ParameterUsage(format!("invalid HTTP frontend location: {error}"))
        })?;
        Ok((RepositorySourceKind::Http, provider.location().to_string()))
    } else {
        let provider = crate::release::repository::provider_for(
            crate::release::repository::ProviderKind::Github,
            location,
        )
        .map_err(|error| {
            InstallError::ParameterUsage(format!("invalid GitHub frontend location: {error}"))
        })?;
        Ok((
            RepositorySourceKind::Github,
            provider.location().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::deployment::config::resolve_active_frontend;
    use crate::deployment::layout;

    use super::*;

    fn setup(name: &str) -> (layout::TerritoryOverride, std::path::PathBuf) {
        let temp =
            std::env::temp_dir().join(format!("lkit-frontend-cmd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = layout::test_territory(&territory);
        (guard, territory)
    }

    #[test]
    fn detects_kind_by_location_shape() {
        assert_eq!(
            normalize_location("https://example.com/ui/").unwrap(),
            (RepositorySourceKind::Http, "https://example.com/ui/".into())
        );
        assert_eq!(
            normalize_location("someone/dark-ui").unwrap(),
            (RepositorySourceKind::Github, "someone/dark-ui".into())
        );
        assert!(normalize_location("not a location").is_err());
        assert!(normalize_location("http://example.com/ui").is_err());
    }

    #[test]
    fn add_activates_and_registers_a_source() {
        let (_guard, territory) = setup("add");
        add(&FrontendAdd {
            id: "custom".into(),
            location: "https://example.com/ui/".into(),
            name: Some("Custom UI".into()),
            activate: true,
        })
        .unwrap();
        let section = load_frontend().unwrap().unwrap();
        assert_eq!(section.active.as_deref(), Some("custom"));
        assert_eq!(section.sources.len(), 1);
        assert_eq!(section.sources[0].location, "https://example.com/ui/");
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn select_switches_and_resolves_the_active_source() {
        let (_guard, territory) = setup("select");
        add(&FrontendAdd {
            id: "a".into(),
            location: "https://a.example.com/ui/".into(),
            name: None,
            activate: false,
        })
        .unwrap();
        add(&FrontendAdd {
            id: "b".into(),
            location: "b/dark-ui".into(),
            name: None,
            activate: false,
        })
        .unwrap();
        assert!(resolve_active_frontend().unwrap().is_none());
        select(&FrontendSelect { id: "b".into() }).unwrap();
        let active = resolve_active_frontend().unwrap().unwrap();
        assert_eq!(active.id, "b");
        assert_eq!(active.kind, RepositorySourceKind::Github);
        select(&FrontendSelect {
            id: crate::deployment::config::FRONTEND_OFFICIAL.into(),
        })
        .unwrap();
        assert!(resolve_active_frontend().unwrap().is_none());
        assert!(select(&FrontendSelect { id: "nope".into() }).is_err());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn remove_requires_selecting_another_source_first() {
        let (_guard, territory) = setup("remove");
        add(&FrontendAdd {
            id: "a".into(),
            location: "https://a.example.com/ui/".into(),
            name: None,
            activate: true,
        })
        .unwrap();
        assert!(matches!(
            remove(&FrontendRemove { id: "a".into() }),
            Err(InstallError::ParameterUsage(_))
        ));
        select(&FrontendSelect {
            id: crate::deployment::config::FRONTEND_OFFICIAL.into(),
        })
        .unwrap();
        remove(&FrontendRemove { id: "a".into() }).unwrap();
        assert!(load_frontend().unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
