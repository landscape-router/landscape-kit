use std::io::Stdout;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub(super) struct ConsoleTerminal {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl ConsoleTerminal {
    pub(super) fn start() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("enable raw mode: {error}"))?;
        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            Hide,
            ClearScreen(ClearType::All),
            MoveTo(0, 0)
        ) {
            let _ = disable_raw_mode();
            // execute! 按序执行、可能已进行到任意一步(如鼠标捕获已开启),
            // 恢复按与 Drop 相同的逆序完整兜底,避免捕获泄漏到 shell。
            let _ = execute!(
                std::io::stdout(),
                Show,
                DisableMouseCapture,
                LeaveAlternateScreen
            );
            return Err(format!("enter alternate screen: {error}"));
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(
                    std::io::stdout(),
                    Show,
                    DisableMouseCapture,
                    LeaveAlternateScreen
                );
                return Err(format!("initialize terminal: {error}"));
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for ConsoleTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            DisableMouseCapture,
            ClearScreen(ClearType::All),
            MoveTo(0, 0),
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
