use std::path::Path;

use super::plan::InstallError;
use super::state::InstallState;
use crate::check::model::Status;
use crate::deployment::runtime::{InstallRuntime, PreflightPolicy};

/// 部署前检查:直接复用 `check` 的结构化检查函数,不解析终端文本。
/// - `error`/`unknown` 停止(端口检查例外:占用者全部被识别为当前受管进程时放行,
///   因为受管 Landscape 会在激活阶段停止并重启);
/// - `warning` 显示后允许继续。
/// - 测试运行时可显式选择 `skip`,用于只验证部署状态机与 service-manager 协议的
///   功能测试;生产运行时始终执行完整检查。
pub(crate) fn run_preflight(
    canonical_root: &Path,
    state: Option<&InstallState>,
    allow_sha_drift: bool,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    if runtime.preflight == PreflightPolicy::Skip {
        return Ok(());
    }
    let report = crate::check::run_all();
    let mut failures = Vec::new();
    for group in &report.groups {
        for result in &group.results {
            let result = match result.id {
                "platform.debian"
                    if runtime.os_release_path
                        != Path::new(crate::check::platform::OS_RELEASE_PATH) =>
                {
                    &crate::check::platform::platform_debian_from(&runtime.os_release_path)
                }
                _ => result,
            };
            match result.status {
                Status::Pass => {}
                Status::Warning => {
                    eprintln!(
                        "install: warning: {}: {} ({})",
                        result.title, result.reason, result.id
                    );
                }
                Status::Error | Status::Unknown => {
                    if managed_occupancy_ok(canonical_root, state, result, allow_sha_drift) {
                        continue;
                    }
                    failures.push(format!("{}: {}", result.id, result.reason));
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(InstallError::Preflight(failures.join("; ")))
    }
}

/// 端口检查占用者全部是当前受管进程时放行;无法识别或非受管占用者不放行。
/// `allow_sha_drift` 用于 `--repair-binary`,允许执行文件摘要漂移的受管进程。
fn managed_occupancy_ok(
    canonical_root: &Path,
    state: Option<&InstallState>,
    result: &crate::check::model::CheckResult,
    allow_sha_drift: bool,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    let checks: &[(super::process::Protocol, u16)] = match result.id {
        "port.dns" => &[
            (super::process::Protocol::Tcp, 53),
            (super::process::Protocol::Udp, 53),
        ],
        "port.http" => &[(super::process::Protocol::Tcp, 6300)],
        "port.https" => &[(super::process::Protocol::Tcp, 6443)],
        _ => return false,
    };
    let pids = super::process::pids_for_ports(checks);
    if pids.is_empty() {
        return false;
    }
    if allow_sha_drift {
        pids.iter().all(|pid| {
            super::process::read_process(*pid)
                .map(|process| {
                    super::process::is_managed(&process, canonical_root, state)
                        || super::process::is_managed_relaxed(&process, canonical_root, state)
                })
                .unwrap_or(false)
        })
    } else {
        pids.iter().all(|pid| {
            super::process::read_process(*pid)
                .map(|process| super::process::is_managed(&process, canonical_root, state))
                .unwrap_or(false)
        })
    }
}
