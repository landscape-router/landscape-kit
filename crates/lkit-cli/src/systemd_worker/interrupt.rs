use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use crate::interaction::presentation::{
    InterruptGuard, OperationResult, OperationScreen, WorkerPresentation,
};

use super::protocol::{WaitOutcome, WorkerResult};
use super::{cleanup_files, systemctl};

#[allow(clippy::too_many_arguments)]
pub(super) fn interrupt_worker(
    systemctl_path: &Path,
    unit_name: &str,
    request_path: &Path,
    result_path: &Path,
    unit_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    presentation_path: &Path,
    credential_path: &Path,
    network_plan_path: &Path,
    interrupt: &InterruptGuard,
    full_screen: bool,
) -> Result<ExitCode, String> {
    if let Err(error) = systemctl(systemctl_path, &["stop", unit_name]) {
        eprintln!(
            "install: {}",
            crate::tr!(
                crate::keys::SYSTEMD_WORKER_STOP_FAILED_WARNING,
                error = error
            )
        );
        return Ok(ExitCode::from(130));
    }
    cleanup_files(&[
        request_path,
        result_path,
        unit_path,
        stdout_path,
        stderr_path,
        presentation_path,
        credential_path,
        network_plan_path,
    ]);
    let _ = systemctl(systemctl_path, &["daemon-reload"]);
    if full_screen {
        crate::interaction::presentation::show_cancelled_screen(interrupt)?;
    }
    Ok(ExitCode::from(130))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wait_for_result(
    systemctl_path: &Path,
    unit_name: &str,
    result_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    presentation_path: &Path,
    interrupt: &InterruptGuard,
    full_screen: bool,
    operation: Box<dyn OperationScreen>,
) -> Result<WaitOutcome, String> {
    let mut stdout = None;
    let mut stderr = None;
    let mut presentation = WorkerPresentation::new(full_screen, operation);
    let mut inactive_polls = 0_u8;
    loop {
        presentation.drain(presentation_path)?;
        if interrupt.requested() {
            if presentation.is_cancellable() {
                presentation.finish();
                return Ok(WaitOutcome::Interrupted);
            }
            interrupt.clear_request();
            presentation.ignore_stop();
        }
        if let Some(action) = presentation.poll_action()? {
            match action {
                crate::interaction::presentation::PresentationAction::Stop => {
                    presentation.finish();
                    return Ok(WaitOutcome::Interrupted);
                }
                crate::interaction::presentation::PresentationAction::Close => unreachable!(),
            }
        }
        drain_log(stdout_path, &mut stdout, false, &mut presentation)?;
        drain_log(stderr_path, &mut stderr, true, &mut presentation)?;
        if result_path.is_file() {
            let content = std::fs::read(result_path)
                .map_err(|error| format!("read worker result: {error}"))?;
            let result: WorkerResult = serde_json::from_slice(&content)
                .map_err(|error| format!("parse worker result: {error}"))?;
            if result.schema_version != 1 {
                return Err(format!(
                    "unsupported worker result schema {}",
                    result.schema_version
                ));
            }
            presentation.drain(presentation_path)?;
            drain_log(stdout_path, &mut stdout, false, &mut presentation)?;
            drain_log(stderr_path, &mut stderr, true, &mut presentation)?;
            let raw_code = result.exit_code.clamp(0, 255) as u8;
            let code = ExitCode::from(raw_code);
            presentation.show_result(code == ExitCode::SUCCESS);
            if full_screen {
                presentation.wait_for_close(interrupt)?;
            }
            announce_completion(presentation.operation(), raw_code);
            presentation.finish();
            return Ok(WaitOutcome::Completed(code));
        }

        if worker_unit_has_stopped(systemctl_path, unit_name)? {
            inactive_polls = inactive_polls.saturating_add(1);
            if inactive_polls >= 10 {
                return Err(format!(
                    "worker unit {unit_name} stopped without writing a result; inspect {} and {}",
                    stdout_path.display(),
                    stderr_path.display()
                ));
            }
        } else {
            inactive_polls = 0;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn worker_unit_has_stopped(systemctl_path: &Path, unit_name: &str) -> Result<bool, String> {
    let output = Command::new(systemctl_path)
        .args(["show", "--property=ActiveState", "--value", unit_name])
        .output()
        .map_err(|error| format!("inspect worker unit {unit_name}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "inspect worker unit {unit_name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(matches!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "inactive" | "failed"
    ))
}

fn drain_log(
    path: &Path,
    file: &mut Option<File>,
    to_stderr: bool,
    presentation: &mut WorkerPresentation,
) -> Result<(), String> {
    if file.is_none() {
        *file = File::open(path).ok();
    }
    let Some(file) = file.as_mut() else {
        return Ok(());
    };
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|error| format!("read worker log {}: {error}", path.display()))?;
    if content.is_empty() {
        return Ok(());
    }
    if presentation.capture_log(&content) {
        return Ok(());
    }
    presentation.before_log();
    if to_stderr {
        eprint!("{content}");
    } else {
        print!("{content}");
        std::io::stdout()
            .flush()
            .map_err(|error| format!("flush delegated stdout: {error}"))?;
    }
    Ok(())
}

/// 操作结束后在普通终端输出明确的结果提示。全屏安装页关闭、或命令模式
/// 委托安装的流式输出结束（可能被忽略）后，用户都能看到操作是否完成。
/// 文案与全屏结果页标题一致（按操作区分，不复用安装文案），前缀为子命令名。
fn announce_completion(operation: &dyn OperationScreen, exit_code: u8) {
    let message = completion_message(operation, exit_code);
    if exit_code == 0 {
        println!("{}: {}", operation.announce_prefix(), message);
    } else {
        eprintln!("{}: {}", operation.announce_prefix(), message);
    }
}

fn completion_message(operation: &dyn OperationScreen, exit_code: u8) -> String {
    if exit_code == 0 {
        crate::tr!(operation.result_key(OperationResult::Success))
    } else {
        format!(
            "{} (exit code {exit_code})",
            crate::tr!(operation.result_key(OperationResult::Failed))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::presentation::{InstallScreen, RestoreScreen};

    #[test]
    fn completion_message_announces_success_and_failure() {
        assert_eq!(
            completion_message(&InstallScreen, 0),
            "Installation complete"
        );
        let failure = completion_message(&InstallScreen, 3);
        assert!(failure.contains("Installation failed"));
        assert!(failure.contains("exit code 3"));
        assert_eq!(completion_message(&RestoreScreen, 0), "Restore complete");
        let failure = completion_message(&RestoreScreen, 3);
        assert!(failure.contains("Restore failed"));
        assert!(failure.contains("exit code 3"));
    }
}
