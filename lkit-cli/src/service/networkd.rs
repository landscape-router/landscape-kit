//! Networkd is masked without enable/disable: its Also= installation graph must stay intact.
use super::{systemd, transaction::HostServiceBefore};
use crate::deployment::plan::InstallError;

pub(crate) fn contains(unit: &str) -> bool {
    (unit.starts_with("systemd-networkd-wait-online@") && unit.ends_with(".service"))
        || matches!(
            unit,
            "systemd-networkd.service"
                | "systemd-networkd.socket"
                | "systemd-networkd-varlink.socket"
                | "systemd-networkd-varlink-metrics.socket"
                | "systemd-networkd-resolve-hook.socket"
                | "systemd-networkd-wait-online.service"
                | "systemd-networkd-wait-online@.service"
        )
}

const WAIT_TEMPLATE: &str = "systemd-networkd-wait-online@.service";

pub(crate) fn inspect_template(
    manager: &systemd::Systemd,
) -> Result<HostServiceBefore, InstallError> {
    let (_, output) = systemd::run_systemctl(
        manager,
        &["list-unit-files", "--full", "--no-legend", WAIT_TEMPLATE],
        None,
    )?;
    template_state(&output)
}

fn template_state(output: &str) -> Result<HostServiceBefore, InstallError> {
    let rows: Vec<_> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let state = match rows.as_slice() {
        [] => "not-found",
        [line] => {
            let mut fields = line.split_whitespace();
            if fields.next() != Some(WAIT_TEMPLATE) {
                return Err(InstallError::Preflight(
                    "unexpected networkd template name".into(),
                ));
            }
            fields
                .next()
                .ok_or_else(|| InstallError::Preflight("missing networkd template state".into()))?
        }
        _ => {
            return Err(InstallError::Preflight(
                "ambiguous networkd template state".into(),
            ));
        }
    };
    Ok(HostServiceBefore {
        unit: WAIT_TEMPLATE.into(),
        installed: state != "not-found",
        active: false,
        enable_state: state.into(),
    })
}

/// Capture instantiated wait-online units as well as the template.
pub(crate) fn instances(manager: &systemd::Systemd) -> Result<Vec<String>, InstallError> {
    let (_, output) = systemd::run_systemctl(
        manager,
        &[
            "list-units",
            "--all",
            "--full",
            "--plain",
            "--no-legend",
            "systemd-networkd-wait-online@*.service",
        ],
        None,
    )?;
    output
        .lines()
        .map(|line| {
            let unit = line.split_whitespace().next().unwrap_or("");
            if !contains(unit) || unit.ends_with("@.service") {
                return Err(InstallError::Preflight(
                    "unexpected networkd wait-online unit".into(),
                ));
            }
            Ok(unit.to_string())
        })
        .collect()
}

fn group(services: &[HostServiceBefore]) -> Vec<&HostServiceBefore> {
    services
        .iter()
        .filter(|s| s.installed && contains(&s.unit))
        .collect()
}

fn stop_with(
    services: &[HostServiceBefore],
    mut run: impl FnMut(&str, &HostServiceBefore) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let services = group(services);
    // Block all activation paths before stopping any member.
    for service in &services {
        run("mask", service)?;
    }
    for service in services.iter().rev() {
        if !service.unit.ends_with("@.service") {
            run("stop", service)?;
        }
    }
    Ok(())
}

fn restore_with(
    services: &[HostServiceBefore],
    mut run: impl FnMut(&str, &HostServiceBefore) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let services = group(services);
    // Unmask the whole group before starting a service with socket dependencies.
    for service in &services {
        run("unmask", service)?;
        if service.enable_state == "masked-runtime" {
            run("unmask-runtime", service)?;
        }
    }
    for service in &services {
        if service.active {
            run("start", service)?;
        }
    }
    // Starting networkd can pull in a socket which was originally inactive.
    for service in &services {
        if !service.active && !service.unit.ends_with("@.service") {
            run("stop", service)?;
        }
    }
    for service in &services {
        match service.enable_state.as_str() {
            "masked" => run("mask", service)?,
            "masked-runtime" => run("mask-runtime", service)?,
            _ => {}
        }
    }
    Ok(())
}

/// A vendor definition can be overlaid with a mask. A local definition at the
/// mask path cannot: reject that layout while capturing, before stopping units.
pub(crate) fn validate_mask_locations(
    manager: &systemd::Systemd,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    if !before.installed {
        return Ok(());
    }
    mask_target(&manager.system_unit_dir, &before.unit)?;
    if before.enable_state == "masked-runtime" {
        mask_target(&manager.run_systemd_dir, &before.unit)?;
    }
    Ok(())
}

fn mask_target(directory: &std::path::Path, unit: &str) -> Result<bool, InstallError> {
    systemd::validate_unit_name(unit)?;
    if !contains(unit) {
        return Err(InstallError::Preflight("invalid networkd mask name".into()));
    }
    let path = directory.join(unit);
    let target = match std::fs::read_link(&path) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(InstallError::Preflight(format!(
                "networkd mask path {} conflicts with an existing entry: {error}",
                path.display()
            )));
        }
    };
    if target != std::path::Path::new("/dev/null") {
        return Err(InstallError::Preflight(format!(
            "refusing to replace networkd mask path {} pointing to {}",
            path.display(),
            target.display()
        )));
    }
    Ok(true)
}

/// Remove only the named mask. `systemctl unmask` also prunes links pointing to
/// that path, which may include startup links that predate our transaction.
fn remove_mask(directory: &std::path::Path, unit: &str) -> Result<(), InstallError> {
    if !mask_target(directory, unit)? {
        return Ok(());
    }
    let path = directory.join(unit);
    std::fs::remove_file(&path).map_err(InstallError::Io)?;
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(InstallError::Io)
}

fn command(
    systemd: &systemd::Systemd,
    verb: &str,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    match verb {
        "unmask" | "unmask-runtime" => {
            let directory = if verb == "unmask-runtime" {
                &systemd.run_systemd_dir
            } else {
                &systemd.system_unit_dir
            };
            remove_mask(directory, &before.unit)?;
            systemd::run_systemctl(systemd, &["daemon-reload"], None)?;
        }
        "mask-runtime" => {
            let verb = verb.trim_end_matches("-runtime");
            systemd::run_systemctl(systemd, &[verb, "--runtime", &before.unit], None)?;
        }
        _ => systemd::unit_command(systemd, verb, &before.unit)?,
    }
    if matches!(verb, "start" | "stop") {
        let active = systemd::unit_query(systemd, "is-active", &before.unit)? == "active";
        if active != (verb == "start") {
            return Err(InstallError::Preflight(format!(
                "networkd unit {} did not reach its requested running state",
                before.unit
            )));
        }
    }
    Ok(())
}

pub(crate) fn stop(
    systemd: &systemd::Systemd,
    services: &[HostServiceBefore],
) -> Result<(), InstallError> {
    if !group(services).is_empty() {
        crate::workflows::operation_control::step(
            crate::daemon_operations::timeline::Step::StoppingNetworkd,
        );
    }
    stop_with(services, |verb, before| command(systemd, verb, before))
}
pub(crate) fn restore(
    systemd: &systemd::Systemd,
    services: &[HostServiceBefore],
) -> Result<(), InstallError> {
    if !group(services).is_empty() {
        crate::workflows::operation_control::step(
            crate::daemon_operations::timeline::Step::RestoringNetworkd,
        );
    }
    restore_with(services, |verb, before| command(systemd, verb, before))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service(unit: &str, active: bool, state: &str) -> HostServiceBefore {
        HostServiceBefore {
            unit: unit.into(),
            installed: true,
            active,
            enable_state: state.into(),
        }
    }
    #[test]
    fn escaped_instance_names_remain_literal_filesystem_names() {
        let dir =
            std::env::temp_dir().join(format!("lkit-networkd-escape-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let unit = r"systemd-networkd-wait-online@enp\x2ds0.service";
        std::os::unix::fs::symlink("/dev/null", dir.join(unit)).unwrap();
        assert!(contains(unit));
        remove_mask(&dir, unit).unwrap();
        assert!(
            systemd::validate_unit_name(r"systemd-networkd-wait-online@enp\xzz.service").is_err()
        );
        assert!(
            systemd::validate_unit_name(r"systemd-networkd-wait-online@enp\x2.service\").is_err()
        );
        assert!(
            systemd::validate_unit_name("systemd-networkd-wait-online@../outside.service").is_err()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_a_mask_preserves_startup_links_and_refuses_foreign_entries() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("lkit-networkd-mask-{}", uuid::Uuid::now_v7()));
        let wants = dir.join("multi-user.target.wants");
        std::fs::create_dir_all(&wants).unwrap();
        let unit = "systemd-networkd.service";
        let path = dir.join(unit);
        let link = wants.join(unit);
        symlink("../systemd-networkd.service", &link).unwrap();
        symlink("/dev/null", &path).unwrap();
        remove_mask(&dir, unit).unwrap();
        remove_mask(&dir, unit).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::path::Path::new("../systemd-networkd.service")
        );
        symlink("/foreign/unit", &path).unwrap();
        assert!(remove_mask(&dir, unit).is_err());
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            std::path::Path::new("/foreign/unit")
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "foreign unit").unwrap();
        assert!(remove_mask(&dir, unit).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "foreign unit");
        assert!(remove_mask(&dir, "systemd-networkd-wait-online@../../outside.service").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn template_is_a_unit_file_without_a_runtime_instance() {
        let snapshot =
            template_state("systemd-networkd-wait-online@.service disabled enabled\n").unwrap();
        assert!(snapshot.installed);
        assert!(!snapshot.active);
        assert_eq!(snapshot.enable_state, "disabled");
        assert!(!template_state("").unwrap().installed);
        assert!(template_state("foreign.service enabled enabled").is_err());
        assert!(template_state(WAIT_TEMPLATE).is_err());
        let mut verbs = vec![];
        stop_with(std::slice::from_ref(&snapshot), |verb, _| {
            verbs.push(verb.to_string());
            Ok(())
        })
        .unwrap();
        restore_with(&[snapshot], |verb, _| {
            verbs.push(verb.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(verbs, ["mask", "unmask"]);
    }
    #[test]
    fn masks_the_whole_group_before_stopping_and_never_changes_startup_links() {
        let mut missing = service("systemd-networkd-varlink.socket", false, "not-found");
        missing.installed = false;
        let services = vec![
            service("systemd-networkd.socket", false, "disabled"),
            service("systemd-networkd.service", true, "enabled"),
            missing,
            service("networking.service", true, "enabled"),
        ];
        let mut calls = vec![];
        stop_with(&services, |verb, s| {
            calls.push((verb.to_string(), s.unit.clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            calls,
            vec![
                ("mask".into(), "systemd-networkd.socket".into()),
                ("mask".into(), "systemd-networkd.service".into()),
                ("stop".into(), "systemd-networkd.service".into()),
                ("stop".into(), "systemd-networkd.socket".into())
            ]
        );
    }
    #[test]
    fn restores_dependencies_before_start_and_reconciles_inactive_sockets_and_masks() {
        let services = vec![
            service("systemd-networkd.socket", false, "masked-runtime"),
            service("systemd-networkd.service", true, "enabled"),
            service("systemd-networkd-wait-online.service", false, "masked"),
        ];
        let mut calls = vec![];
        restore_with(&services, |verb, s| {
            calls.push((verb.to_string(), s.unit.clone()));
            Ok(())
        })
        .unwrap();
        let verbs: Vec<_> = calls.iter().map(|(verb, _)| verb.as_str()).collect();
        assert_eq!(
            verbs,
            [
                "unmask",
                "unmask-runtime",
                "unmask",
                "unmask",
                "start",
                "stop",
                "stop",
                "mask-runtime",
                "mask"
            ]
        );
        assert_eq!(calls[4].1, "systemd-networkd.service");
        assert_eq!(calls[5].1, "systemd-networkd.socket");
    }
    #[test]
    fn failed_mask_does_not_stop_anything_and_recovery_can_be_retried() {
        let services = vec![
            service("systemd-networkd.socket", true, "enabled"),
            service("systemd-networkd.service", true, "enabled"),
        ];
        let mut calls = vec![];
        let result = stop_with(&services, |verb, _| {
            calls.push(verb.to_string());
            if calls.len() == 2 {
                Err(InstallError::Preflight("injected mask failure".into()))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, ["mask", "mask"]);
        let mut first = vec![];
        restore_with(&services, |v, s| {
            first.push((v.to_string(), s.unit.clone()));
            Ok(())
        })
        .unwrap();
        let mut second = vec![];
        restore_with(&services, |v, s| {
            second.push((v.to_string(), s.unit.clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(first, second);
    }
}
