//! Read-only bpffs inspection and best-effort daemon startup preparation.

use std::io;

pub(crate) const MOUNTPOINT: &str = "/sys/fs/bpf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Missing,
    Ready,
    ReadOnly,
    OtherFilesystem,
}

/// Inspect the daemon/check process's mount namespace without creating files.
pub(crate) fn inspect() -> io::Result<State> {
    #[cfg(target_os = "linux")]
    {
        let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
        if !is_mountpoint(&mounts) {
            return Ok(State::Missing);
        }
        let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: the path is NUL-terminated and statfs initializes the output
        // on success. Inspecting the visible filesystem handles stacked mounts.
        if unsafe { libc::statfs(c"/sys/fs/bpf".as_ptr(), fs.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let fs = unsafe { fs.assume_init() };
        const BPF_FS_MAGIC: u32 = 0xcafe4a11;
        if fs.f_type as u32 != BPF_FS_MAGIC {
            return Ok(State::OtherFilesystem);
        }
        let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: as above; statvfs supplies per-mount read-only flags.
        if unsafe { libc::statvfs(c"/sys/fs/bpf".as_ptr(), vfs.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let vfs = unsafe { vfs.assume_init() };
        Ok(if vfs.f_flag & libc::ST_RDONLY != 0 {
            State::ReadOnly
        } else {
            State::Ready
        })
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bpffs requires Linux",
    ))
}

fn is_mountpoint(mountinfo: &str) -> bool {
    mountinfo.lines().any(|line| {
        let Some((fields, filesystem)) = line.split_once(" - ") else {
            return false;
        };
        fields.split_whitespace().nth(4) == Some(MOUNTPOINT)
            && filesystem.split_whitespace().count() >= 3
    })
}

/// Failure is diagnostic only: daemon and flare must remain available.
pub(crate) fn prepare_best_effort() {
    prepare_with(inspect, mount);
}

fn prepare_with(
    inspect: impl FnMut() -> io::Result<State>,
    mount: impl FnOnce() -> io::Result<()>,
) {
    match ensure_with(inspect, mount) {
        Ok(true) => eprintln!("lkit daemon: mounted bpffs at {MOUNTPOINT}"),
        Ok(false) => {}
        Err(error) => eprintln!(
            "lkit daemon: cannot prepare bpffs at {MOUNTPOINT}: {error}; continuing daemon startup"
        ),
    }
}

fn ensure_with(
    mut inspect: impl FnMut() -> io::Result<State>,
    mount: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    match inspect()? {
        State::Ready => return Ok(false),
        State::ReadOnly => return Err(io::Error::other("bpffs is read-only; not remounting")),
        State::OtherFilesystem => {
            return Err(io::Error::other(
                "another filesystem is mounted; not replacing it",
            ));
        }
        State::Missing => {}
    }
    mount()?;
    if inspect()? != State::Ready {
        return Err(io::Error::other(
            "bpffs is not mounted read-write after the mount attempt",
        ));
    }
    Ok(true)
}

fn mount() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::fs::create_dir_all(MOUNTPOINT)?;
        let metadata = std::fs::symlink_metadata(MOUNTPOINT)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "bpffs mountpoint must be a directory, not a symlink",
            ));
        }
        // SAFETY: all string arguments are static NUL-terminated strings and
        // no mount data is required. No external mount utility is necessary.
        let result = unsafe {
            libc::mount(
                c"bpf".as_ptr(),
                c"/sys/fs/bpf".as_ptr(),
                c"bpf".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                std::ptr::null(),
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bpffs requires Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn mountpoint_detection_requires_exact_mountinfo_path() {
        assert!(!is_mountpoint("20 1 0:1 / /sys rw - sysfs sysfs rw\n"));
        assert!(!is_mountpoint(
            "20 1 0:1 / /sys/fs/bpf/child rw - bpf bpf rw\n"
        ));
        assert!(!is_mountpoint("malformed /sys/fs/bpf\n"));
        assert!(is_mountpoint(
            "20 1 0:1 / /sys/fs/bpf rw,nosuid shared:2 - bpf bpf rw\n"
        ));
        // Mountpoint detection does not pretend a different filesystem is bpf;
        // inspect() checks the visible filesystem's magic separately.
        assert!(is_mountpoint(
            "20 1 0:1 / /sys/fs/bpf rw - tmpfs tmpfs rw\n"
        ));
    }

    #[test]
    fn mounted_bpffs_is_untouched() {
        assert!(!ensure_with(|| Ok(State::Ready), || panic!("must not mount again")).unwrap());
    }

    #[test]
    fn missing_bpffs_is_mounted_and_verified_once() {
        let probes = Cell::new(0);
        let mounts = Cell::new(0);
        let result = ensure_with(
            || {
                let before = probes.get();
                probes.set(before + 1);
                Ok(if before == 0 {
                    State::Missing
                } else {
                    State::Ready
                })
            },
            || {
                mounts.set(mounts.get() + 1);
                Ok(())
            },
        );
        assert!(result.unwrap());
        assert_eq!(probes.get(), 2);
        assert_eq!(mounts.get(), 1);
    }

    #[test]
    fn conflicts_and_read_only_mounts_are_not_modified() {
        for state in [State::ReadOnly, State::OtherFilesystem] {
            assert!(ensure_with(|| Ok(state), || panic!("must not overmount or remount")).is_err());
        }
    }

    #[test]
    fn inspection_error_does_not_attempt_mount() {
        assert!(
            ensure_with(
                || Err(io::Error::other("cannot read mountinfo")),
                || panic!("must not mount without knowing the current state")
            )
            .is_err()
        );
    }

    #[test]
    fn mount_success_must_be_verified() {
        assert!(ensure_with(|| Ok(State::Missing), || Ok(())).is_err());
    }

    #[test]
    fn preparation_failures_do_not_prevent_continuing_startup() {
        prepare_with(
            || Ok(State::Missing),
            || Err(io::Error::from_raw_os_error(libc::EPERM)),
        );
        prepare_with(
            || Err(io::Error::other("inspection failed")),
            || panic!("must not mount"),
        );
        prepare_with(|| Ok(State::OtherFilesystem), || panic!("must not mount"));
        prepare_with(|| Ok(State::ReadOnly), || panic!("must not remount"));
    }
}
