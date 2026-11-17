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

fn command(
    systemd: &systemd::Systemd,
    verb: &str,
    before: &HostServiceBefore,
) -> Result<(), InstallError> {
    match verb {
        "mask-runtime" | "unmask-runtime" => {
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
