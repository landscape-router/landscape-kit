use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::plan::InstallError;
use super::state::InstallState;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Protocol {
    Tcp,
    Udp,
}

/// 从 `/proc/net/{tcp,tcp6,udp,udp6}` 解析 `(address, port, inode)` 列表。
/// 地址为十六进制小端 IP 字符串,端口为十六进制。
fn net_sockets(kind: Protocol) -> Vec<(String, u16, u64)> {
    let files = match kind {
        Protocol::Tcp => ["/proc/net/tcp", "/proc/net/tcp6"],
        Protocol::Udp => ["/proc/net/udp", "/proc/net/udp6"],
    };
    let mut sockets = Vec::new();
    for path in files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines().skip(1) {
            let mut fields = line.split_whitespace();
            let Some(_sl) = fields.next() else { continue };
            let Some(local) = fields.next() else { continue };
            let Some(_remote) = fields.next() else {
                continue;
            };
            let Some(_state) = fields.next() else {
                continue;
            };
            let Some(_tx_rx) = fields.next() else {
                continue;
            };
            let Some(_tm_when) = fields.next() else {
                continue;
            };
            let Some(_retrnsmt) = fields.next() else {
                continue;
            };
            let Some(_uid) = fields.next() else { continue };
            let Some(_timeout) = fields.next() else {
                continue;
            };
            let Some(inode) = fields.next() else { continue };
            let Ok(inode) = inode.parse::<u64>() else {
                continue;
            };
            let Some((address, port)) = local.rsplit_once(':') else {
                continue;
            };
            let Ok(port) = u16::from_str_radix(port, 16) else {
                continue;
            };
            sockets.push((address.to_string(), port, inode));
        }
    }
    sockets
}

/// 占用给定端口的进程 PID 集合(通过 `/proc/net` 的 inode 匹配 `/proc/<pid>/fd`)。
pub(crate) fn pids_for_ports(ports: &[(Protocol, u16)]) -> Vec<u32> {
    let mut socket_inodes = HashSet::new();
    for (kind, port) in ports {
        let sockets = net_sockets(*kind);
        socket_inodes.extend(matching_inodes(*port, &sockets));
    }
    if socket_inodes.is_empty() {
        return Vec::new();
    }
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").ok().into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Some(inodes) = fd_inodes(pid) else {
            continue;
        };
        if inodes.iter().any(|inode| socket_inodes.contains(inode)) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn matching_inodes(port: u16, sockets: &[(String, u16, u64)]) -> HashSet<u64> {
    sockets
        .iter()
        .filter_map(|(_, socket_port, inode)| {
            (*socket_port == port && *inode != 0).then_some(*inode)
        })
        .collect()
}

fn fd_inodes(pid: u32) -> Option<Vec<u64>> {
    let mut inodes = Vec::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/fd")).ok()? {
        let entry = entry.ok()?;
        let Ok(link) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let Ok(inode) = link
            .to_string_lossy()
            .trim_start_matches("socket:[")
            .trim_end_matches(']')
            .parse::<u64>()
        else {
            continue;
        };
        inodes.push(inode);
    }
    Some(inodes)
}

/// `/proc/<pid>` 的 Landscape 进程观察。
#[derive(Clone, Debug)]
pub(crate) struct Process {
    pub pid: u32,
    /// `readlink /proc/<pid>/exe` 的完整路径(可能带 ` (deleted)` 后缀)。
    pub exe_link: String,
    /// 通过已打开 `/proc/<pid>/exe` 文件计算的 SHA-256。
    pub exe_sha256: Option<String>,
    /// NUL 分隔的 cmdline 参数。
    pub args: Vec<String>,
}

pub(crate) fn read_process(pid: u32) -> Option<Process> {
    let exe_link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let args = read_cmdline(pid);
    let exe_sha256 = File::open(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|file| {
            let mut reader = std::io::BufReader::new(file);
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = reader.read(&mut buffer).ok()?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Some(hex(&hasher.finalize()))
        });
    Some(Process {
        pid,
        exe_link: exe_link.display().to_string(),
        exe_sha256,
        args,
    })
}

fn read_cmdline(pid: u32) -> Vec<String> {
    let Ok(content) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    content
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

/// 从参数中提取 `--config-dir` 和 `--web` 的值。
pub(crate) fn path_args(args: &[String]) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut config_dir = None;
    let mut web = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config-dir" => {
                if let Some(value) = args.get(index + 1) {
                    config_dir = Some(PathBuf::from(value));
                    index += 1;
                }
            }
            "--web" => {
                if let Some(value) = args.get(index + 1) {
                    web = Some(PathBuf::from(value));
                    index += 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    (config_dir, web)
}

/// 判定进程是否为当前 `lkit` 受管的 Landscape 进程。必须同时满足:
/// - 执行文件摘要等于状态记录;
/// - 执行路径位于真实安装根目录的 `releases/<active-version>`;
/// - 参数中的 data 目录等于 `canonical/data`,静态目录等于 `canonical/current/static`。
pub(crate) fn is_managed(process: &Process, canonical_root: &Path, state: &InstallState) -> bool {
    let expected = format!(
        "{}/releases/{}",
        canonical_root.display(),
        state.active_version
    );
    let Some(sha256) = &process.exe_sha256 else {
        return false;
    };
    if sha256 != &state.assets.webserver.sha256 {
        return false;
    }
    if !is_under(&process.exe_link, canonical_root) || !process.exe_link.starts_with(&expected) {
        return false;
    }
    let (config_dir, web) = path_args(&process.args);
    let expected_data = canonical_root.join("data");
    let expected_static = canonical_root.join("current/static");
    if config_dir.as_deref() != Some(expected_data.as_path()) {
        return false;
    }
    web.as_deref() == Some(expected_static.as_path())
}

fn is_under(path: &str, root: &Path) -> bool {
    Path::new(path).starts_with(root)
}

/// 判定进程是否为指向指定 config 目录的外部 Landscape(非 lkit 受管部署)。
/// 外部实例没有可信摘要,由 cmdline 的 `--config-dir` 与源目录特征文件共同确认;
/// 特征文件校验发生在命令层(源目录必须含 Landscape 特征文件)。
pub(crate) fn is_external_landscape(process: &Process, config_dir: &Path) -> bool {
    let (dir, _web) = path_args(&process.args);
    dir.as_deref() == Some(config_dir)
}

/// 冲突进程检查:给定固定端口,若存在无法确认身份的占用者则返回错误;
/// 返回已确认的受管进程 PID。
pub(crate) fn check_conflicts(
    canonical_root: &Path,
    state: &InstallState,
    ports: &[(Protocol, u16)],
) -> Result<Vec<u32>, InstallError> {
    check_conflicts_with(canonical_root, state, ports, false)
}

/// 与 `check_conflicts` 相同,但允许执行文件摘要漂移:用于 `--repair-binary`,
/// 该场景下运行中的后端二进制摘要与状态记录不一致正是修复目标。
pub(crate) fn check_conflicts_relaxed(
    canonical_root: &Path,
    state: &InstallState,
    ports: &[(Protocol, u16)],
) -> Result<Vec<u32>, InstallError> {
    check_conflicts_with(canonical_root, state, ports, true)
}

fn check_conflicts_with(
    canonical_root: &Path,
    state: &InstallState,
    ports: &[(Protocol, u16)],
    allow_sha_drift: bool,
) -> Result<Vec<u32>, InstallError> {
    let pids = pids_for_ports(ports);
    let mut managed = Vec::new();
    let mut unidentified = Vec::new();
    for pid in pids {
        match read_process(pid) {
            Some(process) if is_managed(&process, canonical_root, state) => managed.push(pid),
            Some(process)
                if allow_sha_drift && is_managed_relaxed(&process, canonical_root, state) =>
            {
                managed.push(pid)
            }
            _ => unidentified.push(pid),
        }
    }
    if !unidentified.is_empty() {
        return Err(InstallError::ProcessConflict(format!(
            "ports {:?} are occupied by unidentified processes {:?}",
            ports, unidentified
        )));
    }
    Ok(managed)
}

/// 执行路径位于受管 release 目录且参数对应本安装,但不校验执行文件摘要。
pub(crate) fn is_managed_relaxed(
    process: &Process,
    canonical_root: &Path,
    state: &InstallState,
) -> bool {
    let expected = format!(
        "{}/releases/{}",
        canonical_root.display(),
        state.active_version
    );
    if !is_under(&process.exe_link, canonical_root) || !process.exe_link.starts_with(&expected) {
        return false;
    }
    let (config_dir, web) = path_args(&process.args);
    let expected_data = canonical_root.join("data");
    let expected_static = canonical_root.join("current/static");
    if config_dir.as_deref() != Some(expected_data.as_path()) {
        return false;
    }
    web.as_deref() == Some(expected_static.as_path())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

use std::fs::File;
use std::io::Read;

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, UdpSocket};

    use super::super::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::*;

    #[test]
    fn retains_all_nonzero_socket_inodes_for_a_port() {
        let sockets = vec![
            ("0100007F".into(), 6443, 101),
            ("0100007F".into(), 6443, 0),
            ("0100007F".into(), 6443, 202),
            ("0100007F".into(), 6300, 303),
        ];

        assert_eq!(matching_inodes(6443, &sockets), HashSet::from([101, 202]));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-process-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn empty_state(canonical: &Path) -> InstallState {
        InstallState {
            schema_version: 1,
            layout_version: 1,
            install_root: canonical.display().to_string(),
            canonical_install_root: canonical.display().to_string(),
            active_version: "0.19.2".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: "a".repeat(64),
                    size: 1,
                },
                static_archive: ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 1,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Pending,
                lock_present: false,
                initialized_at: None,
            },
            service: ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
            last_transaction_id: None,
            committed_at: None,
        }
    }

    #[test]
    fn resolves_port_owners_via_proc_net() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let pids = pids_for_ports(&[(Protocol::Tcp, port)]);
        assert!(
            pids.contains(&std::process::id()),
            "expected own pid {pids:?} for port {port}"
        );

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let pids = pids_for_ports(&[(Protocol::Udp, port)]);
        assert!(
            pids.contains(&std::process::id()),
            "expected own pid {pids:?} for udp port {port}"
        );
        drop(listener);
        drop(socket);
    }

    #[test]
    fn reads_self_process() {
        let process = read_process(std::process::id()).unwrap();
        assert_eq!(process.pid, std::process::id());
        assert!(process.exe_sha256.is_some());
        assert_eq!(process.exe_sha256.unwrap().len(), 64);
        assert!(!process.args.is_empty());
        let link = std::fs::read_link("/proc/self/exe").unwrap();
        assert_eq!(
            process.exe_link,
            link.display().to_string().trim_end_matches(" (deleted)")
        );
    }

    #[test]
    fn extracts_path_args() {
        let args = vec![
            "landscape-webserver".into(),
            "--config-dir".into(),
            "/srv/landscape/data".into(),
            "--web".into(),
            "/srv/landscape/current/static".into(),
        ];
        assert_eq!(
            path_args(&args),
            (
                Some(PathBuf::from("/srv/landscape/data")),
                Some(PathBuf::from("/srv/landscape/current/static"))
            )
        );
        assert_eq!(
            path_args(&["x".into(), "--config-dir".into()]),
            (None, None)
        );
    }

    #[test]
    fn judges_managed_process() {
        let dir = temp_dir("managed");
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let state = empty_state(&canonical);
        let sha = "a".repeat(64);

        let good = Process {
            pid: 1,
            exe_link: format!(
                "{}/releases/0.19.2/landscape-webserver",
                canonical.display()
            ),
            exe_sha256: Some(sha.clone()),
            args: vec![
                "landscape-webserver".into(),
                "--config-dir".into(),
                canonical.join("data").display().to_string(),
                "--web".into(),
                canonical.join("current/static").display().to_string(),
            ],
        };
        assert!(is_managed(&good, &canonical, &state));

        let mut wrong_sha = good.clone();
        wrong_sha.exe_sha256 = Some("c".repeat(64));
        assert!(!is_managed(&wrong_sha, &canonical, &state));
        assert!(is_managed_relaxed(&wrong_sha, &canonical, &state));

        let mut outside = good.clone();
        outside.exe_link = "/opt/other/landscape-webserver".into();
        assert!(!is_managed(&outside, &canonical, &state));
        assert!(!is_managed_relaxed(&outside, &canonical, &state));

        let mut wrong_data = good.clone();
        wrong_data.args[2] = "/elsewhere/data".into();
        assert!(!is_managed(&wrong_data, &canonical, &state));
        assert!(!is_managed_relaxed(&wrong_data, &canonical, &state));

        let mut wrong_web = good.clone();
        wrong_web.args[4] = "/elsewhere/static".into();
        assert!(!is_managed(&wrong_web, &canonical, &state));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
