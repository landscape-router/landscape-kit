use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::interaction::presentation::{
    CloseOutcome, InterruptGuard, OperationResult, OperationScreen, WorkerPresentation,
};

use super::daemon_is_running;
use super::protocol::{WaitOutcome, WorkerResult};

#[allow(clippy::too_many_arguments)]
pub(super) fn wait_for_result(
    result_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    presentation_path: &Path,
    cancel_path: &Path,
    interrupt: &InterruptGuard,
    full_screen: bool,
    operation: Box<dyn OperationScreen>,
) -> Result<WaitOutcome, String> {
    let mut stdout = None;
    let mut stderr = None;
    let mut presentation = WorkerPresentation::new(full_screen, operation);
    let mut daemon_dead_polls = 0_u8;
    loop {
        presentation.drain(presentation_path)?;
        if interrupt.requested() {
            if presentation.is_cancellable() {
                let _ = std::fs::write(cancel_path, b"");
                if presentation.cancel_waits_for_worker() {
                    // 迁移切换的取消由 worker 回滚收尾(旧实例恢复),前台
                    // 继续等待其结果而不是立即返回。
                    presentation.cancel_requested();
                    interrupt.clear_request();
                } else {
                    presentation.finish();
                    return Ok(WaitOutcome::Interrupted);
                }
            } else {
                interrupt.clear_request();
                presentation.ignore_stop();
            }
        }
        if let Some(action) = presentation.poll_action()? {
            match action {
                crate::interaction::presentation::PresentationAction::Stop => {
                    let _ = std::fs::write(cancel_path, b"");
                    if presentation.cancel_waits_for_worker() {
                        presentation.cancel_requested();
                    } else {
                        presentation.finish();
                        return Ok(WaitOutcome::Interrupted);
                    }
                }
                crate::interaction::presentation::PresentationAction::Close => unreachable!(),
                // 确认网络接管只由结果页确认层返回(wait_for_close 内处理)。
                crate::interaction::presentation::PresentationAction::ConfirmTakeover => {
                    unreachable!()
                }
            }
        }
        drain_log(stdout_path, &mut stdout, false, &mut presentation)?;
        drain_log(stderr_path, &mut stderr, true, &mut presentation)?;
        if result_path.is_file() {
            let content = std::fs::read(result_path)
                .map_err(|error| format!("read worker result: {error}"))?;
            let result: WorkerResult = serde_json::from_slice(&content)
                .map_err(|error| format!("parse worker result: {error}"))?;
            if result.schema_version != 2 {
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
            presentation.show_result(raw_code, pending_takeover_confirmation());
            if full_screen
                && matches!(
                    presentation.wait_for_close(interrupt)?,
                    CloseOutcome::ConfirmTakeover
                )
            {
                presentation.finish();
                return Ok(WaitOutcome::ConfirmTakeover);
            }
            announce_completion(presentation.operation(), raw_code);
            presentation.finish();
            return Ok(WaitOutcome::Completed(code));
        }

        if !daemon_is_running() {
            daemon_dead_polls = daemon_dead_polls.saturating_add(1);
            if daemon_dead_polls >= 10 {
                return Err(format!(
                    "the lkit daemon stopped before the operation finished; inspect {} and {}",
                    stdout_path.display(),
                    stderr_path.display()
                ));
            }
        } else {
            daemon_dead_polls = 0;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
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
/// 委托安装的流式输出结束(可能被忽略)后,用户都能看到操作是否完成。
/// 文案与全屏结果页标题一致(按操作区分,不复用安装文案),前缀为子命令名。
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
    } else if exit_code == 130 {
        crate::tr!(operation.result_key(OperationResult::Cancelled))
    } else {
        format!(
            "{} (exit code {exit_code})",
            crate::tr!(operation.result_key(OperationResult::Failed))
        )
    }
}

/// 结果页是否提供「确认网络接管」入口：root 下存在待确认或正在收尾的网络
/// 接管事务（install/reinit 完成后进入该状态）。自动回滚中的事务不再提供
/// 确认,与阻塞屏 `takeover_confirm_allowed` 语义一致。
fn pending_takeover_confirmation() -> bool {
    if unsafe { libc::geteuid() } != 0 {
        return false;
    }
    takeover_confirmation_pending()
}

/// 事务层判定：当前安装根存在待确认/收尾中的网络接管事务。与 euid 检查
/// 分离,便于单元测试用 `test_territory` 构造事务验证。
fn takeover_confirmation_pending() -> bool {
    // 待确认的接管安装尚未提交状态:与 `lkit network` 相同,先从已提交
    // 状态发现根,失败再从未完成事务发现(见 takeover.rs)。
    let root = match crate::deployment::state::discover_landscape_root() {
        Ok(Some(root)) => root,
        _ => {
            match crate::deployment::state::discover_landscape_root_from_unfinished_transaction() {
                Ok(Some(root)) => root,
                _ => return false,
            }
        }
    };
    let Ok(Some(transaction)) = crate::deployment::transaction::find_unfinished(&root) else {
        return false;
    };
    if transaction.network_takeover.is_none() {
        return false;
    }
    matches!(
        transaction.phase,
        crate::deployment::transaction::Phase::AwaitingNetworkConfirmation
            | crate::deployment::transaction::Phase::Finalizing
    )
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
        assert_eq!(completion_message(&RestoreScreen, 130), "Restore cancelled");
    }

    /// 构造一个处于指定阶段的网络接管事务,返回临时目录与领地 guard。
    fn transaction_territory(
        phase: crate::deployment::transaction::Phase,
    ) -> (
        std::path::PathBuf,
        crate::deployment::layout::TerritoryOverride,
    ) {
        let temp = std::env::temp_dir().join(format!(
            "lkit-wait-takeover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = crate::deployment::layout::test_territory(&territory);
        let root = crate::deployment::root::normalize_install_root(&temp).unwrap();
        let mut transaction = crate::deployment::transaction::TransactionFile::new_install(
            &root,
            &semver::Version::new(1, 0, 0),
        )
        .unwrap();
        transaction.phase = phase;
        let id = transaction.transaction_id.clone();
        transaction.network_takeover =
            Some(crate::deployment::transaction::NetworkTakeoverTransaction {
                plan: crate::network::config::NetworkPlan {
                    mode: crate::network::config::NetworkMode::RoutedLan {
                        wan: "ens3".into(),
                        wan_ipv4: None,
                        lan: vec!["ens4".into()],
                        management: "192.168.10.1/24".parse().unwrap(),
                        dhcp_start: "192.168.10.100".parse().unwrap(),
                        dhcp_end: "192.168.10.254".parse().unwrap(),
                    },
                    selected_macs: vec![
                        crate::network::config::SelectedInterface {
                            name: "ens3".into(),
                            mac: "02:00:00:00:00:03".into(),
                        },
                        crate::network::config::SelectedInterface {
                            name: "ens4".into(),
                            mac: "02:00:00:00:00:04".into(),
                        },
                    ],
                },
                host_services: Vec::new(),
                confirmation_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
                rollback_service: format!("lkit-network-{id}-rollback.service"),
                rollback_timer: format!("lkit-network-{id}-rollback.timer"),
                boot_rollback_service: format!("lkit-network-{id}-boot-rollback.service"),
                recovery_binary: "service/lkit-network-recovery".into(),
                pending_state: format!("transactions/{id}/pending-install-state.json"),
            });
        crate::deployment::transaction::persist(&root, &transaction).unwrap();
        (temp, guard)
    }

    #[test]
    fn awaiting_confirmation_transaction_enables_the_result_confirm() {
        let (temp, _guard) = transaction_territory(
            crate::deployment::transaction::Phase::AwaitingNetworkConfirmation,
        );
        assert!(
            takeover_confirmation_pending(),
            "an awaiting network takeover must offer the result confirmation"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rolling_back_transaction_disables_the_result_confirm() {
        let (temp, _guard) =
            transaction_territory(crate::deployment::transaction::Phase::RollingBack);
        assert!(
            !takeover_confirmation_pending(),
            "a rolling back takeover must not offer confirmation"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn no_transaction_disables_the_result_confirm() {
        let temp = std::env::temp_dir().join(format!("lkit-wait-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _guard = crate::deployment::layout::test_territory(&territory);
        let _ = crate::deployment::root::normalize_install_root(&temp).unwrap();
        assert!(!takeover_confirmation_pending());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
