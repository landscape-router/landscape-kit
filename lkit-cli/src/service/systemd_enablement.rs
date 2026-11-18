//! Exact startup memberships for managed systemd services, without process execution.
use super::{SystemRegistration, Systemd, UNIT_NAME, boot, query_registration_at, systemd_error};
use crate::deployment::plan::InstallError;
use crate::service::manager::{Enablement, SystemdStartupLinks};
use std::collections::BTreeSet;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

fn target(systemd: &Systemd, unit: &str, origin: &Path) -> Result<PathBuf, InstallError> {
    if unit == UNIT_NAME {
        boot::copy_path(systemd, origin)
    } else {
        Ok(origin.to_path_buf())
    }
}

pub(super) fn capture(
    systemd: &Systemd,
    unit: &str,
    origin: Option<&Path>,
    boot_id: &str,
) -> Result<Enablement, InstallError> {
    // With no registered origin, no membership can be proved owned.
    let missing = systemd.system_unit_dir.join(".lkit-unregistered-origin");
    let expected = origin.unwrap_or(&missing);
    let copy = if origin.is_some() {
        target(systemd, unit, expected)?
    } else {
        missing.clone()
    };
    let links = boot::startup_links(systemd, unit, expected, &copy)?;
    if origin.is_none() && !links.is_empty() {
        return Err(systemd_error(
            "unregistered service has startup memberships of unknown ownership".into(),
        ));
    }
    let mut snapshot = SystemdStartupLinks {
        version: 1,
        boot_id: boot_id.into(),
        persistent: Vec::new(),
        runtime: Vec::new(),
    };
    for link in links {
        let membership = link
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| systemd_error("non-UTF8 startup membership".into()))?
            .to_owned();
        if link.starts_with(&systemd.system_unit_dir) {
            snapshot.persistent.push(membership);
        } else {
            snapshot.runtime.push(membership);
        }
    }
    snapshot.persistent.sort();
    snapshot.runtime.sort();
    let result = Enablement {
        enabled: !snapshot.persistent.is_empty() || !snapshot.runtime.is_empty(),
        systemd_links: Some(snapshot),
    };
    result.validate()?;
    Ok(result)
}

fn check_directory(path: &Path) -> Result<(), InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallError::Io(error)),
        Ok(_) => Err(systemd_error(format!(
            "startup directory {} is not a real directory",
            path.display()
        ))),
    }
}

fn sync_directory(path: &Path) -> Result<(), InstallError> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(InstallError::Io)
}

pub(super) fn restore(
    systemd: &Systemd,
    unit: &str,
    origin: &Path,
    snapshot: &SystemdStartupLinks,
    boot_id: &str,
) -> Result<(), InstallError> {
    snapshot.validate()?;
    let copy = target(systemd, unit, origin)?;
    let enabled = !snapshot.persistent.is_empty()
        || (snapshot.boot_id == boot_id && !snapshot.runtime.is_empty());
    match query_registration_at(systemd, unit)? {
        SystemRegistration::Symlink { target } if target == origin => {}
        SystemRegistration::Missing if !enabled => {}
        _ => {
            return Err(systemd_error(
                "service registration changed; refusing enablement restoration".into(),
            ));
        }
    }
    // Validate every existing and desired membership before deleting anything.
    let existing = boot::startup_links(systemd, unit, origin, &copy)?;
    let mut desired = BTreeSet::new();
    for (directory, members) in [
        (&systemd.system_unit_dir, &snapshot.persistent),
        (&systemd.run_systemd_dir, &snapshot.runtime),
    ] {
        if directory == &systemd.run_systemd_dir && snapshot.boot_id != boot_id {
            continue;
        }
        for name in members {
            check_directory(directory)?;
            let parent = directory.join(name);
            check_directory(&parent)?;
            desired.insert(parent.join(unit));
        }
    }
    for path in existing {
        if !desired.contains(&path) {
            std::fs::remove_file(&path).map_err(InstallError::Io)?;
            sync_directory(path.parent().unwrap())?;
        }
    }
    for path in desired {
        let parent = path.parent().unwrap();
        match std::fs::DirBuilder::new().mode(0o755).create(parent) {
            Ok(()) => sync_directory(parent.parent().unwrap())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                check_directory(parent)?
            }
            Err(error) => return Err(InstallError::Io(error)),
        }
        boot::atomic_link(&path, &copy)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "systemd_enablement_tests.rs"]
mod tests;
