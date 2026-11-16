pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod reinit;
pub(crate) mod repair;
pub(crate) mod restore;
pub(crate) mod switch;
pub(crate) mod uninstall;
pub(crate) mod update;

pub(crate) use install::InstallScreen;
pub(crate) use migrate::MigrateScreen;
pub(crate) use reinit::ReinitScreen;
pub(crate) use repair::RepairScreen;
pub(crate) use restore::RestoreScreen;
pub(crate) use switch::SwitchScreen;
pub(crate) use uninstall::UninstallScreen;
pub(crate) use update::UpdateScreen;

use super::DownloadState;
use super::OperationPhase;
use crate::interaction::presentation::Frame;

/// 操作结果：成功、失败或取消，用于结果页标题与状态框文案。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationResult {
    Success,
    Failed,
    Cancelled,
}

/// 每个委托操作一个完全独立的页面组件：各自的完整渲染实现（布局与文案
/// 互不共享），以及命令行结束提示所需的结果文案与前缀。将来某个操作需要
/// 不同布局时，只改它自己的文件，不影响其他操作。
pub(crate) trait OperationScreen {
    /// 结果页标题键（完成/失败/取消），供命令行结束提示复用。
    fn result_key(&self, result: OperationResult) -> &'static str;
    /// 停止请求被忽略时的底栏提示键。
    fn stop_ignored_key(&self) -> &'static str;
    /// 该操作完成后是否可能产生待确认的网络接管（install/reinit 为 true，
    /// 其余操作网络已在首次安装时接管，不复用确认窗口）。
    fn takeover_confirmable(&self) -> bool {
        false
    }
    /// 渲染整个操作页（完整独立实现）。
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        frame: &mut Frame<'_>,
        phase: OperationPhase,
        step_progress: Option<(u8, u8)>,
        current: Option<&DownloadState>,
        logs: &[String],
        notice: &str,
        confirming_stop: bool,
        result: Option<OperationResult>,
        takeover_pending: bool,
    );
}

/// 根据操作名选择页面组件，缺省为安装组件。
pub(crate) fn operation_screen(command: &str) -> Box<dyn OperationScreen> {
    match command {
        "install" => Box::new(install::InstallScreen),
        "migrate" => Box::new(migrate::MigrateScreen),
        "switch" => Box::new(switch::SwitchScreen),
        "update" => Box::new(update::UpdateScreen),
        "repair" => Box::new(repair::RepairScreen),
        "restore" => Box::new(restore::RestoreScreen),
        "reinit" => Box::new(reinit::ReinitScreen),
        "uninstall" => Box::new(uninstall::UninstallScreen),
        _ => Box::new(install::InstallScreen),
    }
}

/// 步骤进度条使用的阶段文案（与下载字节进度无关的操作，如 restore）。
pub(crate) fn step_phase_text(phase: OperationPhase) -> String {
    match phase {
        OperationPhase::Preparing => crate::tr!(crate::keys::PRESENTATION_PREPARING),
        OperationPhase::Stopping => crate::tr!(crate::keys::PRESENTATION_STOPPING),
        OperationPhase::Activating => crate::tr!(crate::keys::PRESENTATION_ACTIVATING),
        OperationPhase::Verifying => crate::tr!(crate::keys::PRESENTATION_VERIFYING),
        OperationPhase::Downloading | OperationPhase::Applying => {
            crate::tr!(crate::keys::PRESENTATION_APPLYING_CONFIGURATION)
        }
    }
}

/// 全屏结果页里把网络接管的确认与回滚后果提示行用醒目背景标出。
/// 中英文输出都包含 `lkit network confirm` 命令；等待确认、重新连接和
/// 未确认回滚的提示行分别包含 `confirm`/`确认`。
pub(crate) fn is_confirmation_line(line: &str) -> bool {
    line.contains("confirm") || line.contains("确认")
}
