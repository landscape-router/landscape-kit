use std::path::Path;
use std::time::{Duration, Instant};

use super::plan::InstallError;
use super::process::{self, Protocol};

pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);
pub(crate) const STABLE_OBSERVATION: Duration = Duration::from_secs(10);
pub(crate) const DOCS_PATH: &str = "/api/docs";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PortCheck {
    pub protocol: Protocol,
    pub port: u16,
}

/// `/api/docs` 检查,允许自签名证书。
pub(crate) trait DocsProbe {
    async fn docs_ok(&self) -> bool;
}

/// 健康检查参数，允许测试注入探针与时间配置。
pub(crate) struct HealthOptions<P: DocsProbe> {
    pub docs: P,
    pub ports: Vec<PortCheck>,
    pub startup_timeout: Duration,
    pub stable_duration: Duration,
}

pub(crate) struct HttpsDocsProbe {
    client: reqwest::Client,
    url: String,
}

impl HttpsDocsProbe {
    pub(crate) fn new(base_url: &str) -> Result<Self, InstallError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|error| InstallError::HealthCheck(error.to_string()))?;
        Ok(Self {
            client,
            url: format!("{base_url}{DOCS_PATH}"),
        })
    }
}

impl HealthOptions<HttpsDocsProbe> {
    pub(crate) fn production() -> Result<Self, InstallError> {
        Ok(Self {
            docs: HttpsDocsProbe::new("https://127.0.0.1:6443")?,
            ports: default_port_checks(),
            startup_timeout: STARTUP_TIMEOUT,
            stable_duration: STABLE_OBSERVATION,
        })
    }
}

impl DocsProbe for HttpsDocsProbe {
    async fn docs_ok(&self) -> bool {
        let Ok(response) = self.client.get(&self.url).send().await else {
            return false;
        };
        let status = response.status();
        status.is_success() || status.is_redirection()
    }
}

pub(crate) struct StartupOptions<'a, P: DocsProbe> {
    /// 必须由目标 PID 监听的全部固定端口。
    pub ports: &'a [PortCheck],
    /// 目标 Landscape PID。
    pub expected_pid: u32,
    /// `/api/docs` 检查。
    pub docs: &'a P,
    /// 返回 systemd unit 的 ActiveState(`active`/`inactive`/`failed`/`activating`)。
    pub unit_state: Option<&'a (dyn Fn() -> Option<String> + Send + Sync)>,
    /// 首次安装或 `.lkb` 恢复要求初始化锁与持久配置已生成。
    pub init_required: bool,
    /// 数据目录(init_required 时检查 `landscape_init.lock` 与 `landscape.toml`)。
    pub data_dir: &'a Path,
    /// 启动等待总时长。
    pub startup_timeout: Duration,
    /// 稳定观察时长。
    pub stable_duration: Duration,
}

/// 180 秒启动等待:每秒检查一次,任一条件满足后返回。
pub(crate) async fn wait_for_startup<P: DocsProbe>(
    options: &StartupOptions<'_, P>,
) -> Result<(), InstallError> {
    let started = Instant::now();
    while started.elapsed() < options.startup_timeout {
        if unit_failed(options) {
            return Err(health(
                "systemd unit entered a failed state during startup".into(),
            ));
        }
        if !process_alive(options.expected_pid) {
            return Err(health("landscape process exited during startup".into()));
        }
        if startup_conditions_met(options).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(health(format!(
        "startup did not reach all conditions within {} seconds",
        options.startup_timeout.as_secs()
    )))
}

/// 10 秒稳定观察:进程与端口持续在线,unit 不进入 restarting/failed,
/// 观察结束时再次确认 `/api/docs` 可用。
pub(crate) async fn observe_stable<P: DocsProbe>(
    options: &StartupOptions<'_, P>,
) -> Result<(), InstallError> {
    let started = Instant::now();
    while started.elapsed() < options.stable_duration {
        if unit_unhealthy(options) {
            return Err(health(
                "systemd unit entered a restarting or failed state during observation".into(),
            ));
        }
        if !process_alive(options.expected_pid) {
            return Err(health("landscape process exited during observation".into()));
        }
        if !ports_owned(options) {
            return Err(health(
                "a fixed port stopped being owned by the landscape process during observation"
                    .into(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !options.docs.docs_ok().await {
        return Err(health(
            "/api/docs did not respond at the end of the stable observation".into(),
        ));
    }
    Ok(())
}

async fn startup_conditions_met<P: DocsProbe>(options: &StartupOptions<'_, P>) -> bool {
    ports_owned(options)
        && options.docs.docs_ok().await
        && (!options.init_required || init_files_present(options.data_dir))
}

fn ports_owned<P: DocsProbe>(options: &StartupOptions<'_, P>) -> bool {
    options.ports.iter().all(|check| {
        process::pids_for_ports(&[(check.protocol, check.port)]).contains(&options.expected_pid)
    })
}

fn init_files_present(data_dir: &Path) -> bool {
    data_dir.join("landscape_init.lock").is_file() && data_dir.join("landscape.toml").is_file()
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).is_dir()
}

fn unit_failed<P: DocsProbe>(options: &StartupOptions<'_, P>) -> bool {
    matches!(
        options.unit_state.and_then(|probe| probe()),
        Some(state) if state == "failed"
    )
}

fn unit_unhealthy<P: DocsProbe>(options: &StartupOptions<'_, P>) -> bool {
    matches!(
        options.unit_state.and_then(|probe| probe()),
        Some(state) if state == "failed" || state == "activating"
    )
}

fn health(reason: String) -> InstallError {
    InstallError::HealthCheck(reason)
}

/// 默认固定端口列表:TCP/UDP 53、TCP 6300、TCP 6443。
pub(crate) fn default_port_checks() -> Vec<PortCheck> {
    vec![
        PortCheck {
            protocol: Protocol::Tcp,
            port: 53,
        },
        PortCheck {
            protocol: Protocol::Udp,
            port: 53,
        },
        PortCheck {
            protocol: Protocol::Tcp,
            port: 6300,
        },
        PortCheck {
            protocol: Protocol::Tcp,
            port: 6443,
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-health-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    struct AllOkDocs;

    impl DocsProbe for AllOkDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    struct FailDocs;

    impl DocsProbe for FailDocs {
        async fn docs_ok(&self) -> bool {
            false
        }
    }

    struct ToggleDocs {
        ok: AtomicBool,
    }

    impl DocsProbe for ToggleDocs {
        async fn docs_ok(&self) -> bool {
            self.ok.load(Ordering::Relaxed)
        }
    }

    #[tokio::test]
    async fn startup_succeeds_when_conditions_met() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let data = temp_dir("ok");
        let options = StartupOptions {
            ports: &[PortCheck {
                protocol: Protocol::Tcp,
                port,
            }],
            expected_pid: std::process::id(),
            docs: &AllOkDocs,
            unit_state: Some(&(|| Some("active".into()))),
            init_required: false,
            data_dir: &data,
            startup_timeout: Duration::from_secs(5),
            stable_duration: Duration::from_secs(1),
        };
        wait_for_startup(&options).await.unwrap();
        observe_stable(&options).await.unwrap();
        drop(listener);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn startup_fails_when_port_not_owned() {
        let data = temp_dir("port");
        let options = StartupOptions {
            ports: &[PortCheck {
                protocol: Protocol::Tcp,
                port: 1,
            }],
            expected_pid: std::process::id(),
            docs: &AllOkDocs,
            unit_state: None,
            init_required: false,
            data_dir: &data,
            startup_timeout: Duration::from_millis(1500),
            stable_duration: Duration::from_secs(1),
        };
        assert!(wait_for_startup(&options).await.is_err());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn startup_fails_when_unit_failed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let data = temp_dir("unit");
        let options = StartupOptions {
            ports: &[PortCheck {
                protocol: Protocol::Tcp,
                port,
            }],
            expected_pid: std::process::id(),
            docs: &AllOkDocs,
            unit_state: Some(&(|| Some("failed".into()))),
            init_required: false,
            data_dir: &data,
            startup_timeout: Duration::from_secs(5),
            stable_duration: Duration::from_secs(1),
        };
        assert!(wait_for_startup(&options).await.is_err());
        drop(listener);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn startup_requires_init_files_when_configured() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let data = temp_dir("init");
        let options = StartupOptions {
            ports: &[PortCheck {
                protocol: Protocol::Tcp,
                port,
            }],
            expected_pid: std::process::id(),
            docs: &AllOkDocs,
            unit_state: None,
            init_required: true,
            data_dir: &data,
            startup_timeout: Duration::from_millis(1500),
            stable_duration: Duration::from_secs(1),
        };
        assert!(wait_for_startup(&options).await.is_err());

        std::fs::write(data.join("landscape_init.lock"), b"").unwrap();
        std::fs::write(data.join("landscape.toml"), b"").unwrap();
        let options = StartupOptions {
            startup_timeout: Duration::from_secs(5),
            ..options
        };
        wait_for_startup(&options).await.unwrap();
        drop(listener);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn observation_detects_failing_docs() {
        let docs = ToggleDocs {
            ok: AtomicBool::new(true),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let data = temp_dir("docs");
        let options = StartupOptions {
            ports: &[PortCheck {
                protocol: Protocol::Tcp,
                port,
            }],
            expected_pid: std::process::id(),
            docs: &docs,
            unit_state: None,
            init_required: false,
            data_dir: &data,
            startup_timeout: Duration::from_secs(5),
            stable_duration: Duration::from_millis(2500),
        };
        wait_for_startup(&options).await.unwrap();
        let outcome = std::thread::scope(|scope| {
            let observer = scope.spawn(|| {
                let runtime = tokio::runtime::Runtime::new().unwrap();
                runtime.block_on(observe_stable(&options))
            });
            std::thread::sleep(Duration::from_millis(300));
            docs.ok.store(false, Ordering::Relaxed);
            observer.join().unwrap()
        });
        assert!(outcome.is_err());
        drop(listener);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn provides_default_ports() {
        let checks = default_port_checks();
        assert_eq!(checks.len(), 4);
        assert_eq!(
            checks[0],
            PortCheck {
                protocol: Protocol::Tcp,
                port: 53
            }
        );
        assert_eq!(
            checks[3],
            PortCheck {
                protocol: Protocol::Tcp,
                port: 6443
            }
        );
    }
}
