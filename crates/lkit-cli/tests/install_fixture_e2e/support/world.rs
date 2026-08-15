use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::SYSTEMCTL_FIXTURE;

pub(crate) struct TestWorld {
    root: PathBuf,
    pub(crate) systemctl_config: PathBuf,
    pub(crate) init_config: PathBuf,
}

impl TestWorld {
    pub(crate) fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lkit-cli-fixture-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            systemctl_config: root.join("systemctl.json"),
            init_config: root.join("init.json"),
            root,
        }
    }

    pub(crate) fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl Drop for TestWorld {
    fn drop(&mut self) {
        // 停掉 init 替身拉起的 daemon 进程(OpenRC/sysvinit 场景)。
        if self.init_config.is_file() {
            let Ok(content) = std::fs::read_to_string(&self.init_config) else {
                return;
            };
            let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) else {
                return;
            };
            if let Some(state_dir) = config["state_dir"].as_str() {
                let pid_path = PathBuf::from(state_dir).join("pids/lkit.pid");
                if let Ok(pid) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = pid.trim().parse::<i32>()
                {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
        if self.systemctl_config.is_file() {
            for unit in ["landscape-router.service", "lkit.service"] {
                let _ = Command::new(SYSTEMCTL_FIXTURE)
                    .env(
                        lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                        &self.systemctl_config,
                    )
                    .args(["stop", unit])
                    .output();
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
