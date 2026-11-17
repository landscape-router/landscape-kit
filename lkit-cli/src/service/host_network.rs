//! Host network service snapshots and reversible init backend operations.
use std::path::{Path, PathBuf};
use std::process::Command;

use super::manager::{ManagedService, ServiceManager, ServiceManagerKind, SystemRegistration};
use super::transaction::HostServiceBefore;
use super::{openrc::Openrc, systemd, sysvinit::Sysvinit};
use crate::deployment::plan::InstallError;

fn error(message: impl Into<String>) -> InstallError {
    InstallError::Preflight(message.into())
}

pub(crate) fn names(manager: &dyn ServiceManager) -> &'static [&'static str] {
    match manager.kind() {
        ServiceManagerKind::Systemd => &[
            "systemd-networkd.socket",
            "systemd-networkd-varlink.socket",
            "systemd-networkd-varlink-metrics.socket",
            "systemd-networkd-resolve-hook.socket",
            "systemd-networkd.service",
            "systemd-networkd-wait-online.service",
            "systemd-networkd-wait-online@.service",
            "NetworkManager.service",
            "networking.service",
            "firewalld.service",
            "systemd-resolved.service",
        ],
        _ => &["NetworkManager", "networking", "firewalld"],
    }
}

fn init_dir(manager: &dyn ServiceManager) -> Result<&Path, InstallError> {
    if let Some(backend) = manager.as_any().downcast_ref::<Openrc>() {
        return Ok(&backend.init_d_dir);
    }
    if let Some(backend) = manager.as_any().downcast_ref::<Sysvinit>() {
        return Ok(&backend.init_d_dir);
    }
    Err(error("host network service backend is unavailable"))
}

fn validate_name(name: &str) -> Result<(), InstallError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        || name == "."
        || name == ".."
    {
        return Err(error("invalid host network service name"));
    }
    Ok(())
}

fn command(
    manager: &dyn ServiceManager,
    name: &str,
    verb: &str,
) -> Result<std::process::Output, InstallError> {
    validate_name(name)?;
    let output = if let Some(backend) = manager.as_any().downcast_ref::<Openrc>() {
        Command::new(&backend.rc_service)
            .args([name, verb])
            .output()
    } else {
        Command::new(init_dir(manager)?.join(name))
            .arg(verb)
            .output()
    }
    .map_err(InstallError::Io)?;
    if !output.status.success() && !(verb == "status" && output.status.code() == Some(3)) {
        return Err(error(format!(
            "host service {name} {verb} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StartupLinks {
    backend: ServiceManagerKind,
    links: Vec<(PathBuf, PathBuf)>,
}

fn startup_dirs(manager: &dyn ServiceManager) -> Result<Vec<PathBuf>, InstallError> {
    let etc = init_dir(manager)?
        .parent()
        .ok_or_else(|| error("init directory has no parent"))?;
    if manager.kind() == ServiceManagerKind::Openrc {
        let root = etc.join("runlevels");
        return match std::fs::read_dir(root) {
            Ok(entries) => entries
                .map(|entry| {
                    let entry = entry.map_err(InstallError::Io)?;
                    if !entry.file_type().map_err(InstallError::Io)?.is_dir() {
                        return Err(error("runlevel is not a directory"));
                    }
                    Ok(entry.path())
                })
                .collect(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(err) => Err(InstallError::Io(err)),
        };
    }
    Ok(["0", "1", "2", "3", "4", "5", "6", "S"]
        .map(|level| etc.join(format!("rc{level}.d")))
        .to_vec())
}

fn matching_links(
    manager: &dyn ServiceManager,
    name: &str,
) -> Result<Vec<(PathBuf, PathBuf)>, InstallError> {
    let mut links = vec![];
    for dir in startup_dirs(manager)? {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(InstallError::Io(err)),
        };
        for entry in entries {
            let entry = entry.map_err(InstallError::Io)?;
            let filename = entry.file_name();
            let filename = filename.to_string_lossy();
            let matches = if manager.kind() == ServiceManagerKind::Openrc {
                filename == name
            } else {
                filename.len() == name.len() + 3
                    && matches!(filename.as_bytes()[0], b'S' | b'K')
                    && filename.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
                    && &filename[3..] == name
            };
            if matches {
                links.push((
                    entry.path(),
                    std::fs::read_link(entry.path()).map_err(InstallError::Io)?,
                ));
            }
        }
    }
    links.sort();
    Ok(links)
}

pub(crate) fn startup_links_snapshot(
    manager: &dyn ServiceManager,
    service: ManagedService,
) -> Result<Option<String>, InstallError> {
    let links = matching_links(manager, manager.service_name(service))?;
    serde_json::to_string(&links)
        .map(Some)
        .map_err(InstallError::StateWrite)
}

pub(crate) fn inspect(
    manager: &dyn ServiceManager,
    name: &str,
) -> Result<HostServiceBefore, InstallError> {
    validate_name(name)?;
    if manager.kind() == ServiceManagerKind::Systemd {
        let manager = systemd::downcast(manager)?;
        if name == "systemd-networkd-wait-online@.service" {
            return super::networkd::inspect_template(manager);
        }
        return systemd::inspect_host_service(manager, name);
    }
    let installed = init_dir(manager)?.join(name).exists();
    let active = installed && command(manager, name, "status")?.status.success();
    let links = matching_links(manager, name)?;
    let before = HostServiceBefore {
        unit: name.into(),
        installed,
        active,
        enable_state: serde_json::to_string(&StartupLinks {
            backend: manager.kind(),
            links,
        })
        .map_err(InstallError::StateWrite)?,
    };
    if installed {
        snapshot(manager, &before)?;
    }
    Ok(before)
}

fn snapshot(
    manager: &dyn ServiceManager,
    before: &HostServiceBefore,
) -> Result<StartupLinks, InstallError> {
    validate_name(&before.unit)?;
    let snapshot: StartupLinks = serde_json::from_str(&before.enable_state)
        .map_err(|err| error(format!("invalid host service snapshot: {err}")))?;
    if snapshot.backend != manager.kind() {
        return Err(error("host service backend changed"));
    }
    let dirs = startup_dirs(manager)?;
    for (path, target) in &snapshot.links {
        if !dirs.iter().any(|dir| path.parent() == Some(dir.as_path())) {
            return Err(error("invalid startup link parent"));
        }
        let expected = init_dir(manager)?.join(&before.unit);
        let resolved = path.parent().unwrap().join(target);
        if std::fs::canonicalize(&resolved).map_err(InstallError::Io)?
            != std::fs::canonicalize(expected).map_err(InstallError::Io)?
        {
            return Err(error(
                "startup link does not refer to captured host service",
            ));
        }
        let name = path.file_name().unwrap().to_string_lossy();
        if name != before.unit
            && !(manager.kind() == ServiceManagerKind::Sysvinit
                && name.len() == before.unit.len() + 3
                && matches!(name.as_bytes()[0], b'S' | b'K')
                && name.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
                && name[3..] == before.unit)
        {
            return Err(error("invalid startup link name"));
        }
    }
    Ok(snapshot)
}

pub(crate) fn stop(
    manager: &dyn ServiceManager,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    if manager.kind() == ServiceManagerKind::Systemd {
        return systemd::stop_disable_mask_host_service(systemd::downcast(manager)?, before);
    }
    stop_with(manager, before, |verb| {
        Ok(command(manager, &before.unit, verb)?.status.success())
    })
}

fn stop_with(
    manager: &dyn ServiceManager,
    before: &HostServiceBefore,
    mut run: impl FnMut(&str) -> Result<bool, InstallError>,
) -> Result<(), InstallError> {
    if !before.installed {
        return Ok(());
    }
    let captured = snapshot(manager, before)?;
    let current = matching_links(manager, &before.unit)?;
    if current.iter().any(|link| !captured.links.contains(link)) {
        return Err(error("host service startup links changed since capture"));
    }
    if run("status")? {
        run("stop")?;
        if run("status")? {
            return Err(error("host network service remains active after stop"));
        }
    }
    for (path, _) in matching_links(manager, &before.unit)? {
        std::fs::remove_file(path).map_err(InstallError::Io)?;
    }
    Ok(())
}

pub(crate) fn restore(
    manager: &dyn ServiceManager,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    if manager.kind() == ServiceManagerKind::Systemd {
        return systemd::restore_host_service(systemd::downcast(manager)?, before);
    }
    restore_with(manager, before, |verb| {
        Ok(command(manager, &before.unit, verb)?.status.success())
    })
}

fn restore_with(
    manager: &dyn ServiceManager,
    before: &HostServiceBefore,
    mut run: impl FnMut(&str) -> Result<bool, InstallError>,
) -> Result<(), InstallError> {
    if !before.installed {
        return Ok(());
    }
    let snapshot = snapshot(manager, before)?;
    let current = matching_links(manager, &before.unit)?;
    if current.iter().any(|link| !snapshot.links.contains(link)) {
        return Err(error(
            "refusing to replace changed host service startup links",
        ));
    }
    for (path, _) in current {
        std::fs::remove_file(path).map_err(InstallError::Io)?;
    }
    for (path, target) in snapshot.links {
        std::os::unix::fs::symlink(target, path).map_err(InstallError::Io)?;
    }
    let active = run("status")?;
    if active != before.active {
        run(if before.active { "start" } else { "stop" })?;
        if run("status")? != before.active {
            return Err(error(
                "host network service did not reach its captured running state",
            ));
        }
    }
    Ok(())
}

/// Remove only links owned by this managed service before its init script is
/// unregistered. Validate every link first so a conflict leaves the set intact.
pub(crate) fn remove_managed_startup_links(
    manager: &dyn ServiceManager,
    service: ManagedService,
    origin: &Path,
) -> Result<(), InstallError> {
    match manager.query_registration(service)? {
        SystemRegistration::Missing => return Ok(()),
        SystemRegistration::Symlink { target } if target == origin => {}
        _ => return Err(error("managed service registration has changed")),
    }
    let links = matching_links(manager, manager.service_name(service))?;
    if links.is_empty() {
        return Ok(());
    }
    let expected = std::fs::canonicalize(origin).map_err(InstallError::Io)?;
    for (path, _) in &links {
        if std::fs::canonicalize(path).map_err(InstallError::Io)? != expected {
            return Err(error(
                "refusing to remove foreign managed service startup link",
            ));
        }
    }
    for (path, _) in links {
        std::fs::remove_file(path).map_err(InstallError::Io)?;
    }
    Ok(())
}

/// Networkd must be restored as a group before services can pull in its sockets.
pub(crate) fn restore_all(
    manager: &dyn ServiceManager,
    services: &[HostServiceBefore],
) -> Result<(), InstallError> {
    if manager.kind() == ServiceManagerKind::Systemd {
        super::networkd::restore(systemd::downcast(manager)?, services)?;
    }
    for before in services {
        if manager.kind() != ServiceManagerKind::Systemd || !super::networkd::contains(&before.unit)
        {
            restore(manager, before)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unregister_removes_owned_startup_links_and_rejects_foreign_links() {
        use super::*;
        for openrc in [true, false] {
            let root =
                std::env::temp_dir().join(format!("lkit-managed-links-{}", uuid::Uuid::now_v7()));
            let init = root.join("init.d");
            std::fs::create_dir_all(&init).unwrap();
            let origin = root.join("service");
            std::fs::write(&origin, "fixture").unwrap();
            let manager: Box<dyn ServiceManager> = if openrc {
                Box::new(Openrc {
                    init_d_dir: init.clone(),
                    rc_service: root.join("unused"),
                    rc_update: root.join("unused"),
                    resolv_conf: root.join("resolv.conf"),
                })
            } else {
                Box::new(Sysvinit {
                    init_d_dir: init.clone(),
                    rc_d_glob: root.join("rc?.d"),
                    update_rc_d: root.join("unused"),
                    resolv_conf: root.join("resolv.conf"),
                })
            };
            let service = ManagedService::LandscapeRouter;
            let name = manager.service_name(service);
            let registration = init.join(name);
            std::os::unix::fs::symlink(&origin, &registration).unwrap();
            let mut paths = Vec::new();
            for (level, prefix) in if openrc {
                [("runlevels/boot", ""), ("runlevels/default", "")]
            } else {
                [("rc2.d", "S20"), ("rc0.d", "K80")]
            } {
                let dir = root.join(level);
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join(format!("{prefix}{name}"));
                std::os::unix::fs::symlink(&registration, &path).unwrap();
                paths.push(path);
            }
            let foreign = root.join("foreign");
            std::fs::write(&foreign, "foreign").unwrap();
            std::fs::remove_file(&paths[1]).unwrap();
            std::os::unix::fs::symlink(&foreign, &paths[1]).unwrap();
            assert!(manager.unregister(service, &origin).is_err());
            assert!(registration.exists());
            assert!(paths.iter().all(|path| path.exists()));
            std::fs::remove_file(&paths[1]).unwrap();
            std::os::unix::fs::symlink(&registration, &paths[1]).unwrap();
            for _ in 0..2 {
                manager.unregister(service, &origin).unwrap();
                assert!(std::fs::symlink_metadata(&registration).is_err());
                assert!(
                    paths
                        .iter()
                        .all(|path| std::fs::symlink_metadata(path).is_err())
                );
            }
            assert!(foreign.exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn restoring_running_state_checks_effect_and_can_retry() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("lkit-restore-state-{}", uuid::Uuid::now_v7()));
        let init = root.join("init.d");
        std::fs::create_dir_all(&init).unwrap();
        std::fs::write(init.join("networking"), "fixture").unwrap();
        let manager = Openrc {
            init_d_dir: init,
            rc_service: root.join("unused"),
            rc_update: root.join("unused"),
            resolv_conf: root.join("resolv.conf"),
        };
        for desired in [false, true] {
            let before = HostServiceBefore {
                unit: "networking".into(),
                installed: true,
                active: desired,
                enable_state: serde_json::to_string(&StartupLinks {
                    backend: ServiceManagerKind::Openrc,
                    links: Vec::new(),
                })
                .unwrap(),
            };
            let action = if desired { "start" } else { "stop" };
            let mut calls = Vec::new();
            assert!(
                restore_with(&manager, &before, |verb| {
                    calls.push(verb.to_owned());
                    Ok(if verb == "status" { !desired } else { true })
                })
                .is_err()
            );
            assert_eq!(calls, ["status", action, "status"]);
            let mut active = !desired;
            restore_with(&manager, &before, |verb| {
                if verb == action {
                    active = desired;
                }
                Ok(active)
            })
            .unwrap();
            restore_with(&manager, &before, |verb| {
                assert_eq!(verb, "status");
                Ok(desired)
            })
            .unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn openrc_snapshots_all_runlevels_and_rejects_foreign_links() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("lkit-host-services-{}", uuid::Uuid::now_v7()));
        let init = root.join("init.d");
        std::fs::create_dir_all(&init).unwrap();
        std::fs::write(init.join("networking"), "fixture").unwrap();
        for level in ["boot", "default"] {
            let dir = root.join("runlevels").join(level);
            std::fs::create_dir_all(&dir).unwrap();
            std::os::unix::fs::symlink("../../init.d/networking", dir.join("networking")).unwrap();
        }
        let manager = Openrc {
            init_d_dir: init.clone(),
            rc_service: root.join("unused"),
            rc_update: root.join("unused"),
            resolv_conf: root.join("resolv.conf"),
        };
        let links = matching_links(&manager, "networking").unwrap();
        assert_eq!(links.len(), 2);
        let mut before = HostServiceBefore {
            unit: "networking".into(),
            installed: true,
            active: false,
            enable_state: serde_json::to_string(&StartupLinks {
                backend: ServiceManagerKind::Openrc,
                links,
            })
            .unwrap(),
        };
        assert_eq!(snapshot(&manager, &before).unwrap().links.len(), 2);
        let original = matching_links(&manager, "networking").unwrap();
        stop_with(&manager, &before, |verb| {
            assert_eq!(verb, "status");
            Ok(false)
        })
        .unwrap();
        assert!(matching_links(&manager, "networking").unwrap().is_empty());
        for _ in 0..2 {
            restore_with(&manager, &before, |verb| {
                assert_eq!(verb, "status");
                Ok(false)
            })
            .unwrap();
            assert_eq!(matching_links(&manager, "networking").unwrap(), original);
        }

        before.enable_state = serde_json::to_string(&StartupLinks {
            backend: ServiceManagerKind::Openrc,
            links: vec![(root.join("foreign/networking"), init.join("networking"))],
        })
        .unwrap();
        assert!(snapshot(&manager, &before).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sysv_snapshots_exact_start_and_stop_priorities() {
        use super::*;
        let root = std::env::temp_dir().join(format!("lkit-sysv-links-{}", uuid::Uuid::now_v7()));
        let init = root.join("init.d");
        std::fs::create_dir_all(&init).unwrap();
        std::fs::write(init.join("networking"), "fixture").unwrap();
        for (level, name) in [("2", "S12networking"), ("0", "K88networking")] {
            let dir = root.join(format!("rc{level}.d"));
            std::fs::create_dir_all(&dir).unwrap();
            std::os::unix::fs::symlink("../init.d/networking", dir.join(name)).unwrap();
        }
        let manager = Sysvinit {
            init_d_dir: init,
            rc_d_glob: root.join("rc?.d"),
            update_rc_d: root.join("unused"),
            resolv_conf: root.join("resolv.conf"),
        };
        let links = matching_links(&manager, "networking").unwrap();
        assert_eq!(links.len(), 2);
        assert!(links.iter().any(|(p, _)| p.ends_with("S12networking")));
        assert!(links.iter().any(|(p, _)| p.ends_with("K88networking")));
        let before = HostServiceBefore {
            unit: "networking".into(),
            installed: true,
            active: true,
            enable_state: serde_json::to_string(&StartupLinks {
                backend: ServiceManagerKind::Sysvinit,
                links,
            })
            .unwrap(),
        };
        snapshot(&manager, &before).unwrap();
        let original = matching_links(&manager, "networking").unwrap();
        let mut active = true;
        stop_with(&manager, &before, |verb| {
            if verb == "stop" {
                active = false;
            }
            Ok(active)
        })
        .unwrap();
        assert!(!active);
        assert!(matching_links(&manager, "networking").unwrap().is_empty());
        assert!(
            restore_with(&manager, &before, |_| Err(error(
                "injected service failure"
            )))
            .is_err()
        );
        for _ in 0..2 {
            restore_with(&manager, &before, |verb| {
                if verb == "start" {
                    active = true;
                }
                Ok(active)
            })
            .unwrap();
            assert!(active);
            assert_eq!(matching_links(&manager, "networking").unwrap(), original);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
