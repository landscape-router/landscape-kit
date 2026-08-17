use super::model::{CheckResult, Status};

/// lkit 全局常驻 daemon 的运行状态检查。
///
/// root 会话的安装与生命周期命令都委托给 daemon,daemon 未运行意味着这些
/// 命令必然失败;非 root 会话内联执行,不依赖 daemon。
pub fn run() -> Vec<CheckResult> {
    let root = unsafe { libc::geteuid() } == 0;
    let running = crate::daemon_worker::daemon_is_running();
    vec![lkit_daemon(root, running)]
}

fn lkit_daemon(root: bool, running: bool) -> CheckResult {
    let result = CheckResult::new("service.lkit_daemon", "lkit daemon");
    match (root, running) {
        (_, true) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::CHECK_LKIT_DAEMON_RUNNING),
            crate::tr!(crate::keys::CHECK_LKIT_DAEMON_RUNNING_REASON),
        ),
        (true, false) => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::CHECK_LKIT_DAEMON_NOT_RUNNING),
                crate::tr!(crate::keys::CHECK_LKIT_DAEMON_BLOCKS_DELEGATION),
            )
            .suggestion(crate::tr!(crate::keys::CHECK_LKIT_DAEMON_DEPLOY_HINT)),
        (false, false) => result.set(
            Status::Warning,
            crate::tr!(crate::keys::CHECK_LKIT_DAEMON_NOT_RUNNING),
            crate::tr!(crate::keys::CHECK_LKIT_DAEMON_NON_ROOT_NOTE),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::model::Status;

    #[test]
    fn running_daemon_passes_regardless_of_privilege() {
        assert_eq!(lkit_daemon(true, true).status, Status::Pass);
        assert_eq!(lkit_daemon(false, true).status, Status::Pass);
    }

    #[test]
    fn root_without_daemon_is_an_error_with_deploy_suggestion() {
        let result = lkit_daemon(true, false);
        assert_eq!(result.status, Status::Error);
        assert!(result.suggestion.contains("lkit self install"));
    }

    #[test]
    fn non_root_without_daemon_is_only_a_warning() {
        assert_eq!(lkit_daemon(false, false).status, Status::Warning);
    }
}
