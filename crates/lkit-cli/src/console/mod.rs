mod app;
mod backup;
mod daemon_panel;
mod events;
mod install_form;
mod mirror;
mod network_wizard;
mod preflight;
mod reinit;
mod render;
mod software;
mod terminal;
mod update;
mod widgets;

use self::app::{ConsoleApp, ExitState};
use self::render::render;
use self::terminal::ConsoleTerminal;

use std::io::IsTerminal;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

use crate::commands::Commands;

// 仅测试构建需要：console/tests/* 经 `use super::super::*;` 从 console 命名空间取用这些名字。
// 置于 cfg(test) 下，普通构建保持零未使用导入。
#[cfg(test)]
use backup::BackupListState;
#[cfg(test)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(test)]
use network_wizard::Snapshot;
#[cfg(test)]
use preflight::PreflightState;
#[cfg(test)]
use ratatui::Terminal;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use widgets::{Focus, Hit, Menu};

/// 仅在 console 内部传递,低频构造;`Command` 携带完整命令结构便于分发,保持平坦布局。
#[allow(clippy::large_enum_variant)]
pub(crate) enum ConsoleAction {
    Quit,
    Command {
        command: Commands,
        args: Vec<String>,
    },
}

pub(crate) fn run() -> Result<ConsoleAction, String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(crate::tr!(crate::keys::CONSOLE_TERMINAL_REQUIRED));
    }
    let mut terminal = ConsoleTerminal::start()?;
    let mut app = ConsoleApp::new();
    // 进入控制台即检查 daemon:root 下 daemon 未运行或无法 spawn worker 时
    // 提前在底栏提示,避免用户填写完安装参数、退出控制台委托时才失败。
    match crate::daemon_worker::delegation_block() {
        Some(crate::daemon_worker::DelegationBlock::DaemonNotRunning) => {
            app.notice = crate::tr!(crate::keys::CONSOLE_DAEMON_NOT_RUNNING_NOTICE);
        }
        Some(crate::daemon_worker::DelegationBlock::WorkerSpawnUnavailable) => {
            app.notice = crate::tr!(crate::keys::CONSOLE_DAEMON_SPAWN_UNAVAILABLE_NOTICE);
        }
        None => {}
    }
    loop {
        app.update();
        terminal
            .terminal
            .draw(|frame| render(frame, &mut app))
            .map_err(|error| format!("draw console: {error}"))?;
        if !event::poll(Duration::from_millis(100))
            .map_err(|error| format!("poll terminal event: {error}"))?
        {
            continue;
        }
        match event::read().map_err(|error| format!("read terminal event: {error}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(action) = app.handle_key(key) {
                    return Ok(action);
                }
            }
            Event::Paste(value) => app.handle_paste(&value),
            // 鼠标点击暂时禁用:忽略鼠标事件,终端不再捕获鼠标
            Event::Mouse(_) => {}
            // Event::Mouse(mouse) => {
            //     if let Some(action) = app.handle_mouse(mouse) {
            //         return Ok(action);
            //     }
            // }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
            Event::Key(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
