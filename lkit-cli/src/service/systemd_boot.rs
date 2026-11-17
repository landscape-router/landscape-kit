//! Boot-readable systemd definitions, with installation-local logical ownership.
use super::{SystemRegistration, Systemd, UNIT_NAME, systemd_error, validate_unit};
use crate::deployment::plan::InstallError;
use crate::release::artifacts::hash_str;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

const DIRECTORY: &str = "lkit-managed";
const HEADER: &str = "# lkit managed systemd definition v1 ";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    origin: PathBuf,
    sha256: String,
}

fn conflict(path: &Path) -> InstallError {
    systemd_error(format!(
        "{} is not a verified lkit boot definition; refusing to overwrite or remove it",
        path.display()
    ))
}

fn root(origin: &Path) -> Result<&Path, InstallError> {
    let parent = origin.parent().ok_or_else(|| conflict(origin))?;
    if !origin.is_absolute()
        || origin.file_name().is_none_or(|name| name != UNIT_NAME)
        || parent.file_name().is_none_or(|name| name != "service")
        || origin
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(conflict(origin));
    }
    parent.parent().ok_or_else(|| conflict(origin))
}

fn path_text(path: &Path) -> Result<&str, InstallError> {
    path.to_str()
        .filter(|value| !value.chars().any(char::is_control))
        .ok_or_else(|| conflict(path))
}

pub(super) fn mount_requirement(root: &Path) -> String {
    let value = root
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("RequiresMountsFor=\"{value}\"")
}

fn directory(systemd: &Systemd) -> Result<PathBuf, InstallError> {
    // A target beneath a unit search path is parsed as an alias. Keep this
    // sibling outside /etc/systemd/system so the same name is a linked unit.
    systemd
        .system_unit_dir
        .parent()
        .map(|parent| parent.join(DIRECTORY))
        .ok_or_else(|| conflict(&systemd.system_unit_dir))
}

pub(super) fn copy_path(systemd: &Systemd, origin: &Path) -> Result<PathBuf, InstallError> {
    root(origin)?;
    Ok(directory(systemd)?
        .join(hash_str(path_text(origin)?))
        .join(UNIT_NAME))
}

fn regular_directory(path: &Path) -> Result<(), InstallError> {
    if !std::fs::symlink_metadata(path)
        .map_err(InstallError::Io)?
        .file_type()
        .is_dir()
    {
        return Err(conflict(path));
    }
    Ok(())
}

fn create_directory(path: &Path) -> Result<(), InstallError> {
    match std::fs::create_dir(path) {
        Ok(()) => {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(InstallError::Io)?;
            sync_parent(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(InstallError::Io(error)),
    }
    regular_directory(path)
}

fn regular_file(path: &Path) -> Result<String, InstallError> {
    if !std::fs::symlink_metadata(path)
        .map_err(InstallError::Io)?
        .file_type()
        .is_file()
    {
        return Err(conflict(path));
    }
    std::fs::read_to_string(path).map_err(InstallError::Io)
}

fn suffix(origin: &Path) -> Result<String, InstallError> {
    Ok(format!("\n[Unit]\n{}\n", mount_requirement(root(origin)?)))
}

fn read_copy(systemd: &Systemd, path: &Path) -> Result<(Identity, String), InstallError> {
    regular_directory(&directory(systemd)?)?;
    regular_directory(path.parent().ok_or_else(|| conflict(path))?)?;
    let content = regular_file(path)?;
    let (header, body) = content.split_once('\n').ok_or_else(|| conflict(path))?;
    let identity: Identity =
        serde_json::from_str(header.strip_prefix(HEADER).ok_or_else(|| conflict(path))?)
            .map_err(|_| conflict(path))?;
    if copy_path(systemd, &identity.origin)? != path {
        return Err(conflict(path));
    }
    let source = body
        .strip_suffix(&suffix(&identity.origin)?)
        .ok_or_else(|| conflict(path))?;
    if hash_str(source) != identity.sha256 {
        return Err(conflict(path));
    }
    validate_unit(source, root(&identity.origin)?)?;
    Ok((identity, source.to_owned()))
}

pub(super) fn logical_target(systemd: &Systemd, target: PathBuf) -> Result<PathBuf, InstallError> {
    if target.starts_with(directory(systemd)?) {
        Ok(read_copy(systemd, &target)?.0.origin)
    } else {
        Ok(target)
    }
}

fn raw_registration(systemd: &Systemd) -> Result<SystemRegistration, InstallError> {
    let path = systemd.system_unit_dir.join(UNIT_NAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(SystemRegistration::Symlink {
            target: std::fs::read_link(path).map_err(InstallError::Io)?,
        }),
        Ok(metadata) => Ok(SystemRegistration::Conflict {
            file_type: format!("{:?}", metadata.file_type()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SystemRegistration::Missing)
        }
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn check_registration(
    systemd: &Systemd,
    origin: &Path,
    allow_missing: bool,
) -> Result<(), InstallError> {
    match raw_registration(systemd)? {
        SystemRegistration::Missing if allow_missing => Ok(()),
        SystemRegistration::Symlink { target }
            if logical_target(systemd, target.clone())? == origin =>
        {
            Ok(())
        }
        _ => Err(conflict(&systemd.system_unit_dir.join(UNIT_NAME))),
    }
}

fn absolute_link(path: &Path, target: &Path) -> PathBuf {
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.parent().unwrap().join(target)
    };
    let mut result = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            _ => result.push(component.as_os_str()),
        }
    }
    result
}

/// Only the links for this unit are considered; foreign memberships are never rewritten.
fn startup_links(
    systemd: &Systemd,
    origin: &Path,
    copy: &Path,
) -> Result<Vec<PathBuf>, InstallError> {
    let registration = systemd.system_unit_dir.join(UNIT_NAME);
    let mut links = Vec::new();
    for directory in [&systemd.system_unit_dir, &systemd.run_systemd_dir] {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(InstallError::Io(error)),
        };
        for entry in entries {
            let entry = entry.map_err(InstallError::Io)?;
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".wants")
                && !name.to_string_lossy().ends_with(".requires")
            {
                continue;
            }
            let path = entry.path().join(UNIT_NAME);
            // A redirected directory with a membership cannot be migrated safely.
            let file_type = entry.file_type().map_err(InstallError::Io)?;
            if !file_type.is_dir() {
                if file_type.is_symlink() {
                    match std::fs::symlink_metadata(&path) {
                        Ok(_) => return Err(conflict(&path)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(InstallError::Io(error)),
                    }
                }
                continue;
            }
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(InstallError::Io(error)),
                Ok(metadata) if !metadata.file_type().is_symlink() => return Err(conflict(&path)),
                Ok(_) => {}
            }
            let target =
                absolute_link(&path, &std::fs::read_link(&path).map_err(InstallError::Io)?);
            if target == origin || target == copy || target == registration {
                links.push(path);
            } else {
                return Err(conflict(&path));
            }
        }
    }
    Ok(links)
}

fn sync_parent(path: &Path) -> Result<(), InstallError> {
    std::fs::File::open(path.parent().ok_or_else(|| conflict(path))?)
        .and_then(|parent| parent.sync_all())
        .map_err(InstallError::Io)
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), InstallError> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(InstallError::Io)?;
        file.write_all(content).map_err(InstallError::Io)?;
        file.sync_all().map_err(InstallError::Io)?;
        std::fs::rename(&temporary, path).map_err(InstallError::Io)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn atomic_link(path: &Path, target: &Path) -> Result<(), InstallError> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    symlink(target, &temporary).map_err(InstallError::Io)?;
    let result = std::fs::rename(&temporary, path)
        .map_err(InstallError::Io)
        .and_then(|()| sync_parent(path));
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

struct Publication {
    source: String,
    copy: PathBuf,
    links: Vec<PathBuf>,
    published: Option<String>,
}

/// Validate all ownership and path facts before either reporting a migration
/// requirement or publishing it. Missing managed directories may be created;
/// existing directories must not redirect writes elsewhere.
fn prepare_publication(
    systemd: &Systemd,
    origin: &Path,
    allow_missing: bool,
) -> Result<Publication, InstallError> {
    if origin.canonicalize().map_err(InstallError::Io)? != origin {
        return Err(conflict(origin));
    }
    let source = regular_file(origin)?;
    validate_unit(&source, root(origin)?)?;
    check_registration(systemd, origin, allow_missing)?;
    let copy = copy_path(systemd, origin)?;
    for path in [directory(systemd)?, copy.parent().unwrap().to_path_buf()] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(conflict(&path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    let links = startup_links(systemd, origin, &copy)?;
    let published = match std::fs::symlink_metadata(&copy) {
        Ok(_) => Some(read_copy(systemd, &copy)?.1),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(InstallError::Io(error)),
    };
    Ok(Publication {
        source,
        copy,
        links,
        published,
    })
}

pub(super) fn publish(
    systemd: &Systemd,
    origin: &Path,
    allow_missing: bool,
) -> Result<(), InstallError> {
    let Publication {
        source,
        copy,
        links,
        ..
    } = prepare_publication(systemd, origin, allow_missing)?;
    create_directory(&directory(systemd)?)?;
    create_directory(copy.parent().unwrap())?;
    let identity = Identity {
        origin: origin.to_path_buf(),
        sha256: hash_str(&source),
    };
    let header = serde_json::to_string(&identity).map_err(InstallError::StateWrite)?;
    let content = format!("{HEADER}{header}\n{source}{}", suffix(origin)?);
    atomic_write(&copy, content.as_bytes())?;
    // Keep existing memberships; no enable/disable/start/stop is performed.
    for path in links {
        atomic_link(&path, &copy)?;
    }
    atomic_link(&systemd.system_unit_dir.join(UNIT_NAME), &copy)
}

pub(super) fn needs_refresh(systemd: &Systemd, origin: &Path) -> Result<bool, InstallError> {
    let prepared = prepare_publication(systemd, origin, false)?;
    if !matches!(raw_registration(systemd)?, SystemRegistration::Symlink { target } if target == prepared.copy)
    {
        return Ok(true);
    }
    Ok(prepared.published.as_deref() != Some(prepared.source.as_str()))
}

pub(super) fn unregister(systemd: &Systemd, origin: &Path) -> Result<(), InstallError> {
    check_registration(systemd, origin, true)?;
    let copy = copy_path(systemd, origin)?;
    let owned_copy = match std::fs::symlink_metadata(&copy) {
        Ok(_) => {
            read_copy(systemd, &copy)?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(InstallError::Io(error)),
    };
    let links = startup_links(systemd, origin, &copy)?;
    for path in links
        .into_iter()
        .chain([systemd.system_unit_dir.join(UNIT_NAME)])
    {
        match std::fs::remove_file(&path) {
            Ok(()) => sync_parent(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    if owned_copy {
        std::fs::remove_file(&copy).map_err(InstallError::Io)?;
        sync_parent(&copy)?;
        let _ = std::fs::remove_dir(copy.parent().unwrap());
        let _ = std::fs::remove_dir(directory(systemd)?);
    }
    Ok(())
}

#[cfg(test)]
#[path = "systemd_boot_tests.rs"]
mod tests;
