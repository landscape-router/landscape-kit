use std::path::Path;

use crate::deployment::state::InstallState;
use crate::service::process::{Process, is_managed, is_managed_relaxed, read_process};

use super::model::{CheckResult, Status};

const LISTEN_STATE_TCP: &str = "0A";

/// 端口监听者的身份分类(决定端口检查的结论级别)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenerOwner {
    /// lkit 受管实例:摘要、安装路径与运行参数均与安装状态一致。
    Managed,
    /// 安装路径与参数指向受管安装,但运行中的二进制摘要与安装记录不一致。
    ManagedDigestDrift,
    /// 疑似 Landscape 实例,但不属于本机 lkit 受管安装(外部或遗留部署)。
    ExternalInstance,
    /// 其他进程。
    Foreign,
    /// 占用者信息不可读取或进程已退出。
    Unreadable,
}

#[derive(Debug, Clone)]
struct Listener {
    protocol: &'static str,
    address: String,
    port: u16,
    process: Option<(String, String)>,
    owner: ListenerOwner,
}

pub fn run() -> Vec<CheckResult> {
    vec![
        port_check(
            "port.dns",
            crate::tr!(crate::keys::PORTS_DNS_PORT),
            53,
            true,
        ),
        port_check(
            "port.http",
            crate::tr!(crate::keys::PORTS_HTTP_MANAGEMENT_PORT),
            6300,
            false,
        ),
        port_check(
            "port.https",
            crate::tr!(crate::keys::PORTS_HTTPS_MANAGEMENT_PORT),
            6443,
            false,
        ),
    ]
}

fn port_check(
    id: &'static str,
    title: impl Into<String>,
    port: u16,
    include_udp: bool,
) -> CheckResult {
    // 状态感知:区分"lkit 受管实例"与"非 lkit 的外部实例"需要读取安装状态;
    // 未安装或状态不可读时退化为无状态判定(所有 webserver 按外部实例处理)。
    let state = crate::deployment::state::read_state().ok().flatten();
    let mut listeners = Vec::new();
    let mut read_errors = Vec::new();
    let mut files: Vec<(&str, &'static str, bool)> = vec![
        ("/proc/net/tcp", "tcp", true),
        ("/proc/net/tcp6", "tcp6", true),
    ];
    if include_udp {
        files.push(("/proc/net/udp", "udp", false));
        files.push(("/proc/net/udp6", "udp6", false));
    }
    for (path, protocol, is_tcp) in files {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) => {
                read_errors.push(format!("{path}: {err}"));
                continue;
            }
        };
        for (address, inode) in parse_proc_net(&raw, port, is_tcp) {
            let process = find_process(inode);
            let owner = classify_listener(process.as_ref(), state.as_ref());
            listeners.push(Listener {
                protocol,
                address,
                port,
                process,
                owner,
            });
        }
    }
    let mut result = build_port_result(id, title, port, listeners.clone());
    for error in &read_errors {
        result = result.detail(crate::tr!(
            crate::keys::PORTS_UNABLE_READ_LISTENER_INFORMATION,
            error = error
        ));
    }
    if listeners.is_empty() && !read_errors.is_empty() {
        result = result
            .set(
                Status::Unknown,
                crate::tr!(crate::keys::PORTS_PORT_UNKNOWN, port = port),
                crate::tr!(crate::keys::PORTS_UNABLE_READ_ALL_LISTENER_TABLES),
            )
            .suggestion(crate::tr!(crate::keys::PORTS_RUN_AS_ROOT_FOR_PROC_NET));
    }
    result
}

fn parse_proc_net(raw: &str, port: u16, is_tcp: bool) -> Vec<(String, u64)> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 10 {
                return None;
            }
            let local = parts[1];
            let state = parts[3];
            if is_tcp && state != LISTEN_STATE_TCP {
                return None;
            }
            let (addr, port_hex) = local.rsplit_once(':')?;
            let local_port = u16::from_str_radix(port_hex, 16).ok()?;
            if local_port != port {
                return None;
            }
            let inode = parts[9].parse::<u64>().ok()?;
            Some((addr.to_string(), inode))
        })
        .collect()
}

fn build_port_result(
    id: &'static str,
    title: impl Into<String>,
    port: u16,
    listeners: Vec<Listener>,
) -> CheckResult {
    let mut result = CheckResult::new(id, title);
    if listeners.is_empty() {
        return result.set(
            Status::Pass,
            crate::tr!(crate::keys::PORTS_PORT_NOT_LISTENING, port = port),
            crate::tr!(crate::keys::PORTS_PORT_FREE),
        );
    }
    // 已安装运行的主机上 53/6300/6443 由受管实例长期监听,不属部署冲突;
    // 仅在监听者全部为受管实例(含摘要漂移)时放行,混合占用仍按冲突报告。
    if listeners.iter().all(|listener| {
        matches!(
            listener.owner,
            ListenerOwner::Managed | ListenerOwner::ManagedDigestDrift
        )
    }) {
        let drifted = listeners
            .iter()
            .any(|listener| listener.owner == ListenerOwner::ManagedDigestDrift);
        if drifted {
            result = result.set(
                Status::Warning,
                crate::tr!(
                    crate::keys::PORTS_PORT_HELD_BY_MANAGED_INSTANCE,
                    port = port
                ),
                crate::tr!(crate::keys::PORTS_MANAGED_INSTANCE_DIGEST_DRIFT),
            );
            result.suggestion = crate::tr!(crate::keys::PORTS_RUN_REPAIR_BINARY).to_string();
        } else {
            result = result.set(
                Status::Pass,
                crate::tr!(
                    crate::keys::PORTS_PORT_HELD_BY_MANAGED_INSTANCE,
                    port = port
                ),
                crate::tr!(crate::keys::PORTS_MANAGED_INSTANCE_EXPECTED),
            );
        }
        for listener in &listeners {
            result = result.detail(listener_detail(listener));
        }
        return result;
    }
    // 全部为外部(非 lkit)Landscape 实例时给出接管或停止的指引;
    // 其余(混合占用、其他进程、属主不可读)维持通用冲突文案。
    let all_external = listeners
        .iter()
        .all(|listener| listener.owner == ListenerOwner::ExternalInstance);
    let (reason, suggestion) = if all_external {
        (
            crate::tr!(crate::keys::PORTS_EXTERNAL_INSTANCE_LISTENING).to_string(),
            crate::tr!(crate::keys::PORTS_TAKEOVER_OR_STOP_INSTANCE).to_string(),
        )
    } else {
        (
            crate::tr!(crate::keys::PORTS_ANOTHER_SERVICE_LISTENING).to_string(),
            crate::tr!(crate::keys::PORTS_STOP_SERVICE_OR_MOVE_PORT).to_string(),
        )
    };
    result = result.set(
        Status::Error,
        crate::tr!(crate::keys::PORTS_PORT_OCCUPIED, port = port),
        reason,
    );
    result.suggestion = suggestion;
    for listener in &listeners {
        result = result.detail(listener_detail(listener));
    }
    result
}

fn listener_detail(listener: &Listener) -> String {
    match &listener.process {
        Some((comm, pid)) => crate::tr!(
            crate::keys::PORTS_LISTENER_USED_BY,
            protocol = listener.protocol,
            address = listener.address,
            port = listener.port,
            comm = comm,
            pid = pid
        ),
        None => crate::tr!(
            crate::keys::PORTS_LISTENER_OWNER_UNREADABLE,
            protocol = listener.protocol,
            address = listener.address,
            port = listener.port
        ),
    }
}

fn find_process(inode: u64) -> Option<(String, String)> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid_name = entry.file_name();
        let pid_name = pid_name.to_string_lossy();
        if !pid_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let dir = entry.path();
        let comm = std::fs::read_to_string(dir.join("comm"))
            .ok()
            .map(|c| c.trim().to_string())
            .unwrap_or_default();
        let Ok(fd_dir) = std::fs::read_dir(dir.join("fd")) else {
            continue;
        };
        for fd in fd_dir.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            let Some(rest) = target.strip_prefix("socket:[") else {
                continue;
            };
            let Some(socket_inode) = rest.strip_suffix(']') else {
                continue;
            };
            if socket_inode.parse::<u64>().ok() == Some(inode) {
                return Some((comm, pid_name.to_string()));
            }
        }
    }
    None
}

/// 读取进程信息并分类监听者(仅此函数接触真实进程)。
fn classify_listener(
    process: Option<&(String, String)>,
    state: Option<&InstallState>,
) -> ListenerOwner {
    let Some((comm, pid)) = process else {
        return ListenerOwner::Unreadable;
    };
    let Some(view) = pid.parse::<u32>().ok().and_then(read_process) else {
        return ListenerOwner::Unreadable;
    };
    classify_process(&view, comm, state)
}

/// 纯判定:根据已读取的进程观察与安装状态分类(不触碰文件系统)。
fn classify_process(process: &Process, comm: &str, state: Option<&InstallState>) -> ListenerOwner {
    if let Some(state) = state {
        let canonical = Path::new(&state.canonical_install_root);
        if is_managed(process, canonical, state) {
            return ListenerOwner::Managed;
        }
        if is_managed_relaxed(process, canonical, state) {
            return ListenerOwner::ManagedDigestDrift;
        }
    }
    if looks_like_landscape_webserver(process, comm) {
        ListenerOwner::ExternalInstance
    } else {
        ListenerOwner::Foreign
    }
}

/// 疑似任意 Landscape webserver(含非 lkit 部署)。该判定只影响"外部实例"
/// 分支的文案与建议,不能把端口结论洗白为 pass。
fn looks_like_landscape_webserver(process: &Process, comm: &str) -> bool {
    Path::new(&process.exe_link)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains("landscape-webserver"))
        || is_landscape_comm(comm)
}

/// `/proc/<pid>/comm` 最多 15 个字符（`TASK_COMM_LEN` - 1），
/// `landscape-webserver` 在其中被截断为 `landscape-webse`。
fn is_landscape_comm(comm: &str) -> bool {
    comm.starts_with("landscape-webse")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::deployment::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, ServiceState,
        StateArchitecture, StateServiceManager, WebserverAsset,
    };
    use crate::service::process::Process;

    use super::*;

    const TCP_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20683 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20684 1 0000000000000000 100 0 0 10 0
   2: 00000000:1388 00000000:0000 06 00000000:00000000 00:00000000 00000000     0        0 20685 1 0000000000000000 100 0 0 10 0";

    const UDP_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 20686 1 0000000000000000 100 0 0 10 0";

    const TCP6_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:189C 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20687 1 0000000000000000 100 0 0 10 0";

    #[test]
    fn parses_listening_tcp_entries() {
        let found = parse_proc_net(TCP_SAMPLE, 53, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], ("00000000".to_string(), 20683));
    }

    #[test]
    fn ignores_non_listening_tcp_states() {
        assert!(parse_proc_net(TCP_SAMPLE, 5000, true).is_empty());
    }

    #[test]
    fn filters_by_port() {
        assert!(parse_proc_net(TCP_SAMPLE, 80, true).is_empty());
    }

    #[test]
    fn parses_udp_entries() {
        let found = parse_proc_net(UDP_SAMPLE, 53, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], ("00000000".to_string(), 20686));
    }

    #[test]
    fn parses_tcp6_entries() {
        let found = parse_proc_net(TCP6_SAMPLE, 6300, true);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0],
            ("00000000000000000000000000000000".to_string(), 20687)
        );
    }

    fn listener(owner: ListenerOwner, comm: &str, pid: &str) -> Listener {
        Listener {
            protocol: "tcp",
            address: "00000000".to_string(),
            port: 53,
            process: Some((comm.to_string(), pid.to_string())),
            owner,
        }
    }

    #[test]
    fn free_port_reports_pass() {
        let result = build_port_result("port.test", "test port", 1234, Vec::new());
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.value, "1234 not listening");
    }

    #[test]
    fn occupied_port_reports_error_with_details() {
        let listeners = vec![listener(ListenerOwner::Foreign, "named", "123")];
        let result = build_port_result("port.test", "test port", 53, listeners);
        assert_eq!(result.status, Status::Error);
        assert_eq!(result.value, "53 occupied");
        assert!(
            result
                .details
                .iter()
                .any(|d| d.contains("named") && d.contains("123"))
        );
    }

    #[test]
    fn occupied_port_reports_error_without_process() {
        let listeners = vec![Listener {
            protocol: "udp",
            address: "00000000".to_string(),
            port: 53,
            process: None,
            owner: ListenerOwner::Unreadable,
        }];
        let result = build_port_result("port.test", "test port", 53, listeners);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .details
                .iter()
                .any(|d| d.contains("owner information is unreadable"))
        );
    }

    #[test]
    fn managed_listeners_report_pass() {
        let listeners = vec![listener(ListenerOwner::Managed, "landscape-webse", "378")];
        let result = build_port_result("port.test", "test port", 6300, listeners);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.value, "6300 held by the lkit-managed instance");
        assert!(
            result
                .details
                .iter()
                .any(|d| d.contains("landscape-webse") && d.contains("378"))
        );
    }

    #[test]
    fn managed_digest_drift_reports_warning_with_repair_hint() {
        let listeners = vec![listener(
            ListenerOwner::ManagedDigestDrift,
            "landscape-webse",
            "378",
        )];
        let result = build_port_result("port.test", "test port", 6300, listeners);
        assert_eq!(result.status, Status::Warning);
        assert_eq!(result.value, "6300 held by the lkit-managed instance");
        assert!(result.suggestion.contains("lkit repair --repair-binary"));
    }

    #[test]
    fn external_instance_reports_error_with_takeover_hint() {
        let listeners = vec![listener(
            ListenerOwner::ExternalInstance,
            "landscape-webse",
            "77",
        )];
        let result = build_port_result("port.test", "test port", 6300, listeners);
        assert_eq!(result.status, Status::Error);
        assert_eq!(result.value, "6300 occupied");
        assert!(result.reason.contains("not managed by lkit"));
        assert!(result.suggestion.contains("lkit migrate"));
    }

    #[test]
    fn mixed_managed_and_external_listeners_report_error() {
        let listeners = vec![
            listener(ListenerOwner::Managed, "landscape-webse", "378"),
            listener(ListenerOwner::ExternalInstance, "landscape-webse", "77"),
        ];
        let result = build_port_result("port.test", "test port", 53, listeners);
        assert_eq!(result.status, Status::Error);
        assert_eq!(result.value, "53 occupied");
        assert!(result.suggestion.contains("Stop the service"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-check-ports-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixture_state(canonical: &Path) -> InstallState {
        InstallState {
            schema_version: 1,
            layout_version: 2,
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
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("c".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: None,
        }
    }

    fn fixture_process(canonical: &Path, sha: &str) -> Process {
        Process {
            pid: 378,
            exe_link: canonical
                .join("releases/0.19.2/landscape-webserver")
                .display()
                .to_string(),
            exe_sha256: Some(sha.to_string()),
            args: vec![
                "landscape-webserver".into(),
                "--config-dir".into(),
                canonical.join("data").display().to_string(),
                "--web".into(),
                canonical.join("current/static").display().to_string(),
            ],
        }
    }

    #[test]
    fn classify_marks_managed_process() {
        let canonical = temp_dir("managed").join("landscape");
        let state = fixture_state(&canonical);
        let process = fixture_process(&canonical, &state.assets.webserver.sha256);
        assert_eq!(
            classify_process(&process, "landscape-webse", Some(&state)),
            ListenerOwner::Managed
        );
    }

    #[test]
    fn classify_marks_digest_drift() {
        let canonical = temp_dir("drift").join("landscape");
        let state = fixture_state(&canonical);
        let process = fixture_process(&canonical, &"d".repeat(64));
        assert_eq!(
            classify_process(&process, "landscape-webse", Some(&state)),
            ListenerOwner::ManagedDigestDrift
        );
    }

    #[test]
    fn classify_marks_external_instance_outside_canonical_root() {
        let canonical = temp_dir("external").join("landscape");
        let state = fixture_state(&canonical);
        let process = Process {
            pid: 77,
            exe_link: "/opt/landscape-webserver".into(),
            exe_sha256: None,
            args: vec!["landscape-webserver".into()],
        };
        assert_eq!(
            classify_process(&process, "landscape-webse", Some(&state)),
            ListenerOwner::ExternalInstance
        );
    }

    #[test]
    fn classify_marks_foreign_process() {
        let canonical = temp_dir("foreign").join("landscape");
        let state = fixture_state(&canonical);
        let process = Process {
            pid: 55,
            exe_link: "/usr/sbin/named".into(),
            exe_sha256: None,
            args: vec!["named".into()],
        };
        assert_eq!(
            classify_process(&process, "named", Some(&state)),
            ListenerOwner::Foreign
        );
    }

    #[test]
    fn classify_treats_missing_state_as_external() {
        let process = Process {
            pid: 77,
            exe_link: "/root/landscape-webserver".into(),
            exe_sha256: None,
            args: Vec::new(),
        };
        assert_eq!(
            classify_process(&process, "landscape-webse", None),
            ListenerOwner::ExternalInstance
        );
    }

    #[test]
    fn unreadable_process_classifies_as_unreadable() {
        assert_eq!(classify_listener(None, None), ListenerOwner::Unreadable);
    }

    #[test]
    fn landscape_comm_prefix_matches_truncated_name() {
        assert!(is_landscape_comm("landscape-webse"));
        assert!(!is_landscape_comm("named"));
        assert!(!is_landscape_comm("landscape-flare"));
    }

    #[test]
    fn read_state_under_temp_territory_classifies_managed_listener() {
        let root = temp_dir("territory");
        let _guard = crate::deployment::layout::test_territory(&root);

        // 没有状态文件:未安装,不能确认受管实例。
        assert!(crate::deployment::state::read_state().unwrap().is_none());

        let canonical = root.join("landscape");
        let state = fixture_state(&canonical);
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::write(
            root.join("state").join("install-state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();

        let loaded = crate::deployment::state::read_state().unwrap().unwrap();
        let process = fixture_process(&canonical, &state.assets.webserver.sha256);
        assert_eq!(
            classify_process(&process, "landscape-webse", Some(&loaded)),
            ListenerOwner::Managed
        );
    }
}
