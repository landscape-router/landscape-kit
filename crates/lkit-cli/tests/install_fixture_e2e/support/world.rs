use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::SYSTEMCTL_FIXTURE;

pub(crate) struct TestWorld {
    root: PathBuf,
    pub(crate) systemctl_config: PathBuf,
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
            root,
        }
    }

    pub(crate) fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl Drop for TestWorld {
    fn drop(&mut self) {
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
