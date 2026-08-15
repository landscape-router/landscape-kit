use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::interaction::interactive::DAEMON_WORKER_TTY_ENV;
use crate::interaction::presentation::PRESENTATION_EVENTS_ENV;

use super::protocol::{
    RemoveFile, WorkerRequest, WorkerResult, validate_credential_path, validate_network_plan_path,
    validate_request_path, write_private_json,
};

const CANCEL_GRACE_POLLS: u32 = 25;

/// daemon 侧执行一次委托请求:读取并认领请求文件、以子进程执行命令、
/// 响应 cancel 文件并写入结果。返回命令的业务退出码。
pub(crate) fn execute_request(request_path: &Path) -> i32 {
    match execute_request_inner(request_path) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("lkit daemon worker: {error}");
            1
        }
    }
}

fn execute_request_inner(request_path: &Path) -> Result<i32, String> {
    validate_request_path(request_path)?;
    let content = std::fs::read(request_path)
        .map_err(|error| format!("read worker request {}: {error}", request_path.display()))?;
    let request: WorkerRequest = serde_json::from_slice(&content)
        .map_err(|error| format!("parse worker request {}: {error}", request_path.display()))?;
    if request.schema_version != 2 {
        return Err(format!(
            "unsupported worker request schema {}",
            request.schema_version
        ));
    }
    // 认领请求,避免重启后的 daemon 或并发周期重复执行。
    std::fs::remove_file(request_path)
        .map_err(|error| format!("claim worker request {}: {error}", request_path.display()))?;
    let _credential = match request.credential_path.as_deref() {
        Some(path) => {
            validate_credential_path(path)?;
            Some(RemoveFile::new(path))
        }
        None => None,
    };
    let _network_plan = match request.network_plan_path.as_deref() {
        Some(path) => {
            validate_network_plan_path(path)?;
            Some(RemoveFile::new(path))
        }
        None => None,
    };

    let executable =
        std::env::current_exe().map_err(|error| format!("resolve worker executable: {error}"))?;
    // 请求由常驻 daemon 执行,已经脱离发起者的会话;进程组隔离仍保证
    // 取消时能连同孙进程一起终止。
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let mut command = Command::new(executable);
    command
        .args(&request.args)
        .env_clear()
        .envs(request.environment)
        .current_dir(&request.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(terminal) = request.terminal {
        command.env(DAEMON_WORKER_TTY_ENV, terminal);
    } else {
        command.env_remove(DAEMON_WORKER_TTY_ENV);
    }
    command.env(PRESENTATION_EVENTS_ENV, &request.presentation_path);
    // 子进程成为新进程组长,取消时以 kill(-pgid) 覆盖孙进程。
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("run delegated lkit command: {error}"))?;
    let cancel_path = request.cancel_path.clone();
    let exit_code = wait_for_child(&mut child, &cancel_path);
    write_private_json(
        &request.result_path,
        &WorkerResult {
            schema_version: 2,
            exit_code,
        },
    )?;
    let _ = std::fs::remove_file(&cancel_path);
    Ok(exit_code)
}

fn wait_for_child(child: &mut std::process::Child, cancel_path: &Path) -> i32 {
    let mut cancelled = false;
    let mut grace_polls = 0_u32;
    loop {
        if !cancelled && cancel_path.is_file() {
            cancelled = true;
            // SIGINT 无法可靠送达无终端子进程组,直接以 SIGTERM 请求退出。
            let _ = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGTERM) };
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("wait for delegated lkit command: {error}"))
            .ok()
            .flatten()
        {
            return status.code().unwrap_or(1);
        }
        if cancelled {
            grace_polls += 1;
            if grace_polls >= CANCEL_GRACE_POLLS {
                let _ = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
