use std::path::Path;
use std::process::{Command, Output};
use std::sync::Mutex;

mod harness;
mod ports;
mod pty;
mod repo;
mod transactions;
mod world;

pub(crate) use self::harness::InstallHarness;
pub(crate) use self::pty::{Pty, attach_pty};
pub(crate) use self::repo::{RepositoryServer, repository_files, repository_files_for};
pub(crate) use self::transactions::{
    read_only_transaction, transaction_count, transaction_of_operation,
};
use self::world::TestWorld;

pub(crate) const VERSION: &str = "1.2.3";
pub(crate) const LKIT: &str = env!("CARGO_BIN_EXE_lkit");
pub(crate) const LANDSCAPE_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-landscape-fixture");
const SYSTEMCTL_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-test-systemctl");
pub(crate) static E2E_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn systemctl(world: &TestWorld, args: &[&str]) -> Output {
    Command::new(SYSTEMCTL_FIXTURE)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &world.systemctl_config,
        )
        .args(args)
        .output()
        .unwrap()
}

pub(crate) fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_json(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

pub(crate) fn assert_host_services_masked(harness: &InstallHarness, units: &[&str]) {
    for unit in units {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("masked").is_file(), "{unit} was not masked");
        assert!(!state.join("active").exists(), "{unit} remains active");
        assert!(!state.join("enabled").exists(), "{unit} remains enabled");
        assert!(harness.host.join("units").join(unit).is_file());
    }
}

pub(crate) fn assert_host_services_restored(harness: &InstallHarness, units: &[&str]) {
    for unit in units {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("active").is_file(), "{unit} was not restarted");
        assert!(state.join("enabled").is_file(), "{unit} was not re-enabled");
        assert!(!state.join("masked").exists(), "{unit} remains masked");
        assert!(harness.host.join("units").join(unit).is_file());
    }
}
