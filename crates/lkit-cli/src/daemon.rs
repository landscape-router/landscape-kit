//! lkit 常驻服务。
//!
//! Phase B 只提供最小骨架:写入 pidfile、处理 SIGTERM/SIGINT 干净退出。
//! 回滚引擎(事务接管、启动失败看门狗、启动时中断恢复)在 Phase C 接入。
//!
//! 单元入口与 [`ServiceManager::render_definition`] 的 `LkitDaemon` 定义一致:
//! `lkit daemon --config-dir <install-root>/data`,pidfile 固定为
//! `<install-root>/run/lkit.pid`(config-dir 的父目录即安装根)。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;

use crate::deployment::plan::InstallError;

pub(crate) const PIDFILE_NAME: &str = "lkit.pid";

#[derive(Debug, Args)]
pub struct Daemon {
    /// Landscape data 目录(安装根目录的 data/,pidfile 写入其父目录的 run/)
    #[arg(long, value_name = "PATH")]
    pub config_dir: PathBuf,
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_termination(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// 运行 lkit 常驻服务直到收到 SIGTERM/SIGINT。
pub(crate) fn run(args: &Daemon) -> ExitCode {
    run_with_config_dir(&args.config_dir)
}

pub(crate) fn run_with_config_dir(config_dir: &Path) -> ExitCode {
    match run_inner(config_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(config_dir: &Path) -> Result<(), InstallError> {
    let canonical = std::fs::canonicalize(config_dir).map_err(|error| {
        InstallError::Io(std::io::Error::new(
            error.kind(),
            format!("resolve config dir {}: {error}", config_dir.display()),
        ))
    })?;
    if !canonical.is_dir() {
        return Err(InstallError::ParameterUsage(format!(
            "config dir {} is not a directory",
            canonical.display()
        )));
    }
    let root = canonical.parent().ok_or_else(|| {
        InstallError::ParameterUsage(format!(
            "config dir {} has no install root parent",
            canonical.display()
        ))
    })?;
    let run_dir = root.join("run");
    std::fs::create_dir_all(&run_dir).map_err(InstallError::Io)?;
    let pidfile = run_dir.join(PIDFILE_NAME);
    write_pidfile(&pidfile)?;

    unsafe {
        let handler: extern "C" fn(libc::c_int) = handle_termination;
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
    while !SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let _ = std::fs::remove_file(&pidfile);
    Ok(())
}

fn write_pidfile(pidfile: &Path) -> Result<(), InstallError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Ok(existing) = std::fs::read_to_string(pidfile)
        && let Ok(pid) = existing.trim().parse::<u32>()
        && process_alive(pid)
    {
        return Err(InstallError::ProcessConflict(format!(
            "another lkit daemon is already running with pid {pid}"
        )));
    }
    let tmp = pidfile.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    writeln!(file, "{}", std::process::id()).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, pidfile).map_err(InstallError::Io)?;
    Ok(())
}

fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
