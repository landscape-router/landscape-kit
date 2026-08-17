use std::process::Command;

use super::MirrorError;

/// 换源成功后刷新软件包索引,让新源立即生效,避免后续 apt/dnf/pacman 操作
/// 基于过期或空索引报 "Unable to locate package" 之类的错误。
/// `stream` 为 true 时输出流到终端(CLI 模式);false 时捕获输出(TUI worker 模式)。
/// 子进程设置 PDEATHSIG,父进程(lkit)退出时自动终止。
pub(crate) fn refresh_index(family: super::Family, stream: bool) -> Result<(), MirrorError> {
    // 测试注入:跳过真实包管理器进程。
    if super::paths().skip_refresh {
        return Ok(());
    }
    let (program, args): (&str, &[&str]) = match family {
        super::Family::Debian | super::Family::Ubuntu => ("apt-get", &["update"]),
        super::Family::Fedora | super::Family::Rocky | super::Family::Alma => {
            ("dnf", &["makecache"])
        }
        super::Family::Arch => ("pacman", &["-Sy"]),
    };
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec 在 fork 后 exec 前运行;仅设置 PDEATHSIG,
        // 不触碰进程状态,不调用异步不安全函数。
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    if stream {
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(MirrorError::Message(format!(
                "{program} exited with status {status}"
            )))
        }
    } else {
        let output = command.output()?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            Err(MirrorError::Message(if stderr.is_empty() {
                format!(
                    "{program} exited with status {}",
                    output.status.code().unwrap_or(1)
                )
            } else {
                format!("{program}: {stderr}")
            }))
        }
    }
}
