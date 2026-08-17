use std::fs::File;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

use super::screens::{InstallScreen, OperationResult};
use super::worker::WorkerPresentation;

const SIGINT_MODE_NONE: u8 = 0;
const SIGINT_MODE_DIRECT: u8 = 1;
const SIGINT_MODE_DELEGATED: u8 = 2;
const SIGINT_MODE_CONSOLE: u8 = 3;
const TERMINAL_RESTORE: &[u8] = b"\x1b[0m\x1b[?25h\r\n";
const CONSOLE_RESTORE: &[u8] = b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H\x1b[?1049l\r\n";
static SIGINT_MODE: AtomicU8 = AtomicU8::new(SIGINT_MODE_NONE);
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);
static TERMINAL_FD: AtomicI32 = AtomicI32::new(-1);
static ORIGINAL_TERMIOS: SavedTermios = SavedTermios::new();

struct SavedTermios(std::cell::UnsafeCell<MaybeUninit<libc::termios>>);

// The value is initialized before the SIGINT action is installed and remains
// read-only until that action has been restored.
unsafe impl Sync for SavedTermios {}

impl SavedTermios {
    const fn new() -> Self {
        Self(std::cell::UnsafeCell::new(MaybeUninit::uninit()))
    }
}

pub(crate) struct InterruptGuard {
    previous: libc::sigaction,
    terminal: Option<TerminalState>,
}

impl InterruptGuard {
    pub(crate) fn install(delegated: bool) -> Result<Self, String> {
        let mode = if delegated {
            SIGINT_MODE_DELEGATED
        } else {
            SIGINT_MODE_DIRECT
        };
        Self::install_mode(mode)
    }

    pub(crate) fn install_console() -> Result<Self, String> {
        Self::install_mode(SIGINT_MODE_CONSOLE)
    }

    fn install_mode(mode: u8) -> Result<Self, String> {
        SIGINT_MODE
            .compare_exchange(SIGINT_MODE_NONE, mode, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "a Ctrl+C handler is already active".to_string())?;
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
        let terminal = (!crate::interaction::interactive::is_non_interactive())
            .then(TerminalState::capture)
            .flatten();
        if let Some(terminal) = &terminal {
            unsafe {
                (*ORIGINAL_TERMIOS.0.get()).write(terminal.original);
            }
            TERMINAL_FD.store(terminal.file.as_raw_fd(), Ordering::SeqCst);
        }
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle_sigint as *const () as usize;
        action.sa_flags = 0;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous) } != 0 {
            TERMINAL_FD.store(-1, Ordering::SeqCst);
            SIGINT_MODE.store(SIGINT_MODE_NONE, Ordering::SeqCst);
            return Err(format!(
                "install terminal recovery handler: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { previous, terminal })
    }

    pub(crate) fn requested(&self) -> bool {
        SIGINT_RECEIVED.load(Ordering::SeqCst)
    }

    pub(crate) fn clear_request(&self) {
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
    }
}

struct TerminalState {
    file: File,
    original: libc::termios,
}

impl TerminalState {
    fn capture() -> Option<Self> {
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open("/dev/tty")
            .ok()
            .or_else(|| {
                [libc::STDERR_FILENO, libc::STDIN_FILENO, libc::STDOUT_FILENO]
                    .into_iter()
                    .find(|fd| unsafe { libc::isatty(*fd) } == 1)
                    .and_then(|fd| {
                        let duplicate = unsafe { libc::dup(fd) };
                        (duplicate >= 0).then(|| unsafe { File::from_raw_fd(duplicate) })
                    })
            })?;
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut original) } != 0 {
            return None;
        }
        Some(Self { file, original })
    }

    fn restore(&self) {
        unsafe {
            libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original);
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous, std::ptr::null_mut());
        }
        if let Some(terminal) = &self.terminal {
            terminal.restore();
        }
        TERMINAL_FD.store(-1, Ordering::SeqCst);
        SIGINT_MODE.store(SIGINT_MODE_NONE, Ordering::SeqCst);
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
    }
}

extern "C" fn handle_sigint(_signal: libc::c_int) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
    let terminal_fd = TERMINAL_FD.load(Ordering::SeqCst);
    let mode = SIGINT_MODE.load(Ordering::SeqCst);
    unsafe {
        if terminal_fd >= 0 {
            libc::tcsetattr(
                terminal_fd,
                libc::TCSANOW,
                (*ORIGINAL_TERMIOS.0.get()).as_ptr(),
            );
            let restore = if mode == SIGINT_MODE_CONSOLE {
                CONSOLE_RESTORE
            } else {
                TERMINAL_RESTORE
            };
            libc::write(terminal_fd, restore.as_ptr().cast(), restore.len());
        }
        if mode == SIGINT_MODE_DIRECT || mode == SIGINT_MODE_CONSOLE {
            libc::_exit(130);
        }
    }
}

pub(crate) fn show_cancelled_screen(interrupt: &InterruptGuard) -> Result<(), String> {
    let mut presentation = WorkerPresentation::new(true, Box::new(InstallScreen));
    presentation.result = Some(OperationResult::Cancelled);
    presentation.render_screen();
    presentation.wait_for_close(interrupt)?;
    presentation.finish();
    Ok(())
}
