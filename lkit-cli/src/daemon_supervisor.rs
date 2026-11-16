//! Init-independent daemon supervision. The init service owns this process;
//! workers remain owned by the daemon and are not killed on a daemon restart.
use crate::daemon::Daemon;
use std::process::ExitCode;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Backoff {
    failures: usize,
}
impl Backoff {
    fn next(&mut self, uptime: Duration) -> Duration {
        if uptime >= Duration::from_secs(300) {
            self.failures = 0;
        }
        let seconds = [1, 3, 5, 10][self.failures.min(3)];
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(seconds)
    }
}

pub(crate) async fn run(args: &Daemon) -> ExitCode {
    match supervise(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit supervisor: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn supervise(_args: &Daemon) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let _instance_lock = instance_lock("supervisor.lock")?;
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let executable = std::env::current_exe()?;
    let mut backoff = Backoff::default();
    loop {
        let started = Instant::now();
        let mut command = tokio::process::Command::new(&executable);
        command.arg("daemon");
        #[cfg(feature = "test-support")]
        if let Some(path) = &_args.test_runtime {
            command.arg("--test-runtime").arg(path);
        }
        match command.spawn() {
            Ok(mut child) => {
                tokio::select! {
                    biased;
                    _ = term.recv() => { stop(&mut child).await?; return Ok(()); }
                    _ = interrupt.recv() => { stop(&mut child).await?; return Ok(()); }
                    status = child.wait() => { eprintln!("lkit daemon exited: {status:?}"); }
                }
            }
            Err(error) => eprintln!("lkit daemon spawn failed: {error}"),
        }
        let delay = backoff.next(started.elapsed());
        eprintln!("lkit daemon restarting in {}s", delay.as_secs());
        tokio::select! {
            biased;
            _ = term.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn stop(child: &mut tokio::process::Child) -> std::io::Result<()> {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    // Let an active recovery finish before the supervisor exits. Init backends
    // must not kill this process before the daemon has released its lock.
    child.wait().await?;
    Ok(())
}

pub(crate) fn instance_lock(name: &str) -> std::io::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let directory = crate::deployment::layout::territory_run_dir();
    std::fs::create_dir_all(&directory)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(directory.join(name))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_forever_at_ten_seconds_and_resets_after_stability() {
        let mut policy = Backoff::default();
        for expected in [1, 3, 5, 10, 10, 10, 10] {
            assert_eq!(policy.next(Duration::ZERO).as_secs(), expected);
        }
        assert_eq!(policy.next(Duration::from_secs(299)).as_secs(), 10);
        assert_eq!(policy.next(Duration::from_secs(300)).as_secs(), 1);
        assert_eq!(policy.next(Duration::ZERO).as_secs(), 3);
        assert_eq!(Backoff::default().next(Duration::ZERO).as_secs(), 1);
    }
}
