use std::sync::mpsc::{self, TryRecvError};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::ConsoleApp;
use super::render::{panel_block, register_dialog_hits, register_modal_hits};

/// daemon 部署后台线程的最终结果:成功返回与 CLI 相同的结果消息,失败返回错误文本。
pub(super) type DeployResult = Result<String, String>;

impl ConsoleApp {
    /// Overview 是否可执行 daemon 部署:daemon 未运行时显示「部署」动作行。
    /// 非 root 会话也能看到动作行,确认后得到与 CLI 相同的 root 权限错误提示。
    pub(super) fn daemon_deploy_available(&self) -> bool {
        !crate::daemon_worker::daemon_is_running()
    }

    /// 在后台线程执行 `lkit self install`(与 CLI 相同的 root 检查、安装锁与
    /// systemd 语义),结果经 channel 回传,由 `poll_daemon_deploy` 展示。
    /// 控制台不另起 lkit 进程、不解析 CLI 文本输出。
    pub(super) fn start_daemon_deploy(&mut self) -> Result<(), String> {
        if self.deploy_daemon.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || {
                crate::commands::lkit_self::install_daemon().map_err(|error| error.to_string())
            });
            let _ = sender.send(result);
        });
        self.deploy_daemon = Some(receiver);
        Ok(())
    }

    /// 部署完成或线程断开后收起进度,结果写入底栏;成功后 Overview 状态行
    /// 在下一帧自动变为运行中。
    pub(super) fn poll_daemon_deploy(&mut self) {
        let Some(receiver) = &self.deploy_daemon else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.deploy_daemon = None;
                self.deploy_daemon_confirming = false;
                self.notice = crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_WORKER_STOPPED);
                return;
            }
        };
        self.deploy_daemon = None;
        self.deploy_daemon_confirming = false;
        match result {
            Ok(message) => {
                self.notice = message;
                // 部署成功后预检报告已过期(daemon 检查此前报告 error 并挡住了
                // 安装表单),自动重跑,报告更新后表单门禁自然放行。
                self.preflight.restart();
            }
            Err(error) => self.notice = error,
        }
    }
}

pub(crate) fn render_daemon_deploy_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if !app.deploy_daemon_confirming {
        return;
    }
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 10.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_PLAN),
                Style::default().fg(Color::DarkGray),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_PRESS_ENTER),
                Style::default().fg(Color::Green),
            ),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_PRESS_ESC),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_TITLE))),
        area,
    );
}

pub(crate) fn render_daemon_deploy_progress(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if app.deploy_daemon.is_none() {
        return;
    }
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 5.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_RUNNING),
                Style::default().fg(Color::Cyan),
            ),
            Line::raw(""),
        ])
        .wrap(Wrap { trim: true })
        .block(panel_block(
            &crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_TITLE),
            true,
        )),
        area,
    );
}
