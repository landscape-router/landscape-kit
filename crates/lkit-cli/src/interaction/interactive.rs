use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use super::plan::InstallError;

const TTY_PATH: &str = "/dev/tty";
pub(crate) const SYSTEMD_WORKER_TTY_ENV: &str = "LKIT_INTERNAL_SYSTEMD_WORKER_TTY";

/// 所有交互输入输出只通过终端设备,不读取 stdin,避免消费管道数据。
/// 通常使用 `/dev/tty`; systemd worker 直接打开前端传入的原终端设备,
/// 避免 transient unit 争用 controlling terminal。
pub(crate) struct Tty {
    file: File,
}

impl Tty {
    pub(crate) fn open() -> Result<Self, InstallError> {
        let path = std::env::var_os(SYSTEMD_WORKER_TTY_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(TTY_PATH));
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(&path)
            .map_err(|_| {
                InstallError::NonInteractive(format!(
                    "cannot open {}; interactive confirmation is not possible",
                    path.display()
                ))
            })?;
        Ok(Self { file })
    }

    /// 显示提示并要求用户输入完整 ASCII `yes`。空输入、其他内容、
    /// EOF 或中断都视为拒绝并返回 `Ok(false)`。
    pub(crate) fn confirm(&mut self, prompt: &str) -> Result<bool, InstallError> {
        self.write_prompt(prompt)?;
        let line = self.read_line()?;
        Ok(line == "yes")
    }

    /// 隐藏输入密码并要求二次确认。失败时不输出任何内容。
    pub(crate) fn read_password(&mut self, prompt: &str) -> Result<String, InstallError> {
        let first = self.read_password_once(&format!("{prompt}: "))?;
        let second = self.read_password_once(&format!("{prompt} (again): "))?;
        if first != second {
            return Err(InstallError::InvalidPassword(
                "password confirmation does not match".into(),
            ));
        }
        Ok(first)
    }

    fn read_password_once(&mut self, prompt: &str) -> Result<String, InstallError> {
        let fd = self.file.as_raw_fd();
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(InstallError::InvalidPassword(
                "cannot read terminal settings".into(),
            ));
        }
        let mut termios = original;
        termios.c_lflag &= !libc::ECHO;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            return Err(InstallError::InvalidPassword(
                "cannot disable terminal echo".into(),
            ));
        }
        self.write_prompt(prompt)?;
        let result = self.read_line();
        let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) };
        let line = result?;
        if line.is_empty() {
            return Err(InstallError::InvalidPassword(
                "interactive password must not be empty".into(),
            ));
        }
        Ok(line)
    }

    fn write_prompt(&mut self, prompt: &str) -> Result<(), InstallError> {
        let result = self
            .file
            .write_all(prompt.as_bytes())
            .and_then(|_| self.file.flush());
        match result {
            // pty 主端已关闭时写入返回 EIO/EPIPE,视为输入流已结束。
            Err(error)
                if error.raw_os_error() == Some(libc::EIO)
                    || error.raw_os_error() == Some(libc::EPIPE) =>
            {
                Ok(())
            }
            Err(error) => Err(InstallError::NonInteractive(error.to_string())),
            Ok(()) => Ok(()),
        }
    }

    /// 读取一行,去掉行尾的 `\n`(兼容 `\r\n`)。EOF 或 pty 主端关闭时的
    /// EIO 都视为输入结束,返回已读内容(可能为空)。
    fn read_line(&mut self) -> Result<String, InstallError> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match self.file.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    line.push(byte[0]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) => {
                    return Err(InstallError::NonInteractive(error.to_string()));
                }
            }
        }
        while matches!(line.last(), Some(b'\r')) {
            line.pop();
        }
        String::from_utf8(line)
            .map_err(|_| InstallError::NonInteractive("input is not valid UTF-8".into()))
    }
}

/// 在无法打开 `/dev/tty` 时返回 `NonInteractive` 错误,否则要求完整 `yes`。
pub(crate) fn confirm(prompt: &str) -> Result<bool, InstallError> {
    Tty::open()?.confirm(prompt)
}

/// 交互式隐藏密码输入,要求二次确认。
pub(crate) fn read_password(prompt: &str) -> Result<String, InstallError> {
    Tty::open()?.read_password(prompt)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    /// 通过 openpty 提供真实的伪终端,验证隐藏输入与行读取。
    struct Pty {
        master: File,
        slave: File,
    }

    impl Pty {
        fn open() -> Self {
            let mut master: libc::c_int = 0;
            let mut slave: libc::c_int = 0;
            let mut name: [libc::c_char; 64] = [0; 64];
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        name.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            Self {
                master: unsafe { std::os::fd::FromRawFd::from_raw_fd(master) },
                slave: unsafe { std::os::fd::FromRawFd::from_raw_fd(slave) },
            }
        }

        fn read_all(&mut self) -> String {
            let mut output = String::new();
            let _ = self.master.read_to_string(&mut output);
            output
        }

        fn write(&mut self, bytes: &[u8]) {
            self.master.write_all(bytes).unwrap();
        }
    }

    #[test]
    fn open_fails_without_controlling_terminal() {
        assert!(matches!(Tty::open(), Err(InstallError::NonInteractive(_))));
    }

    #[test]
    fn confirm_requires_full_yes() {
        for (input, expected) in [
            ("yes\n", true),
            ("yes\r\n", true),
            ("y\n", false),
            ("YES\n", false),
            ("yes please\n", false),
            ("\n", false),
            ("no\n", false),
        ] {
            let mut pty = Pty::open();
            pty.write(input.as_bytes());
            let master = pty.master;
            let mut tty = Tty { file: pty.slave };
            let result = tty.confirm("proceed?").unwrap();
            assert_eq!(result, expected, "input {input:?}");
            drop(master);
        }
    }

    #[test]
    fn confirm_eof_is_rejection() {
        let pty = Pty::open();
        let master = pty.master;
        drop(master);
        let mut tty = Tty { file: pty.slave };
        assert!(!tty.confirm("proceed?").unwrap());
    }

    #[test]
    fn password_requires_match() {
        let mut pty = Pty::open();
        pty.write(b"Secret123\nSecret123\n");
        let master = pty.master;
        let mut tty = Tty { file: pty.slave };
        assert_eq!(tty.read_password("password").unwrap(), "Secret123");
        drop(master);

        let mut pty = Pty::open();
        pty.write(b"Secret123\nOther123\n");
        let master = pty.master;
        let mut tty = Tty { file: pty.slave };
        assert!(matches!(
            tty.read_password("password"),
            Err(InstallError::InvalidPassword(_))
        ));
        drop(master);
    }

    #[test]
    fn password_rejects_empty() {
        let mut pty = Pty::open();
        pty.write(b"\n\n");
        let master = pty.master;
        let mut tty = Tty { file: pty.slave };
        assert!(matches!(
            tty.read_password("password"),
            Err(InstallError::InvalidPassword(_))
        ));
        drop(master);
    }

    #[test]
    fn password_echo_is_disabled_on_pty() {
        let pty = Pty::open();
        let master = pty.master;
        let writer = std::thread::spawn(move || {
            let mut master = master;
            let mut output = String::new();
            let mut byte = [0u8; 1];
            loop {
                let read = master.read(&mut byte).unwrap();
                if read == 0 {
                    break;
                }
                output.push(byte[0] as char);
                if output.ends_with("password: ") {
                    break;
                }
            }
            master.write_all(b"Secret123\n").unwrap();
            loop {
                let read = master.read(&mut byte).unwrap();
                if read == 0 {
                    break;
                }
                output.push(byte[0] as char);
                if output.ends_with("password (again): ") {
                    break;
                }
            }
            master.write_all(b"Secret123\n").unwrap();
            let mut rest = String::new();
            let _ = master.read_to_string(&mut rest);
            output.push_str(&rest);
            output
        });
        let mut tty = Tty { file: pty.slave };
        tty.read_password("password").unwrap();
        drop(tty);
        let output = writer.join().unwrap();
        assert!(!output.contains("Secret123"), "password leaked: {output}");
    }
}
