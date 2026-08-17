use std::ffi::CStr;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) struct Pty {
    pub(crate) master: File,
    pub(crate) slave: File,
    pub(crate) slave_path: PathBuf,
}

impl Pty {
    pub(crate) fn open() -> Self {
        let mut master = 0;
        let mut slave = 0;
        let mut name = [0 as libc::c_char; 128];
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    name.as_mut_ptr(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        let slave_path = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_str()
            .unwrap()
            .into();
        Self {
            master: unsafe { File::from_raw_fd(master) },
            slave: unsafe { File::from_raw_fd(slave) },
            slave_path,
        }
    }

    pub(crate) fn read_until(&mut self, expected: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut descriptor = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout_ms = remaining.as_millis().min(100) as libc::c_int;
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if ready < 0 {
                panic!("poll pty: {}", std::io::Error::last_os_error());
            }
            if ready == 0 || descriptor.revents & libc::POLLIN == 0 {
                continue;
            }
            let mut buffer = [0_u8; 4096];
            let size = self.master.read(&mut buffer).unwrap();
            output.extend_from_slice(&buffer[..size]);
            if String::from_utf8_lossy(&output).contains(expected) {
                return String::from_utf8_lossy(&output).into_owned();
            }
        }
        panic!(
            "timed out waiting for {expected:?}; pty output:\n{}",
            String::from_utf8_lossy(&output)
        );
    }

    pub(crate) fn echo_enabled(&self) -> bool {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut termios) },
            0
        );
        termios.c_lflag & libc::ECHO != 0
    }
}

pub(crate) fn attach_pty(command: &mut Command, pty: &Pty) {
    command
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
