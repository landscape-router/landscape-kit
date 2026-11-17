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

pub(super) const WAIT_TEMPLATE: &str = "systemd-networkd-wait-online@.service";

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

pub(super) fn template_state(output: &str) -> Result<HostServiceBefore, InstallError> {
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
