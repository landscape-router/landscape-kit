use super::*;
use std::os::unix::fs::symlink;

const BOOT: &str = "11111111-1111-4111-8111-111111111111";
const NEXT_BOOT: &str = "22222222-2222-4222-8222-222222222222";

struct Fixture {
    directory: PathBuf,
    systemd: Systemd,
    origin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("lkit-enablement-{}", uuid::Uuid::now_v7()));
        let systemd = Systemd {
            systemctl: directory.join("never-execute-systemctl"),
            system_unit_dir: directory.join("units"),
            run_systemd_dir: directory.join("runtime-units"),
            pid1_is_systemd: true,
            resolv_conf: directory.join("resolv.conf"),
        };
        for directory in [&systemd.system_unit_dir, &systemd.run_systemd_dir] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let root = directory.join("installation");
        let origin = root.join("service").join(UNIT_NAME);
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::write(&origin, super::super::render_unit(&root)).unwrap();
        boot::publish(&systemd, &origin, true).unwrap();
        Self {
            directory,
            systemd,
            origin,
        }
    }

    fn link(&self, runtime: bool, name: &str) -> PathBuf {
        let base = if runtime {
            &self.systemd.run_systemd_dir
        } else {
            &self.systemd.system_unit_dir
        };
        let directory = base.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(UNIT_NAME);
        symlink(boot::copy_path(&self.systemd, &self.origin).unwrap(), &path).unwrap();
        path
    }

    fn snapshot(&self, boot: &str) -> Enablement {
        capture(&self.systemd, UNIT_NAME, Some(&self.origin), boot).unwrap()
    }

    fn restore(&self, before: &Enablement, boot: &str) -> Result<(), InstallError> {
        restore(
            &self.systemd,
            UNIT_NAME,
            &self.origin,
            before.systemd_links.as_ref().unwrap(),
            boot,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn runtime_restore_removes_transaction_persistent_links_and_is_idempotent() {
    let fixture = Fixture::new();
    let runtime = fixture.link(true, "multi-user.target.wants");
    let before = fixture.snapshot(BOOT);
    std::fs::remove_file(&runtime).unwrap();
    let introduced = fixture.link(false, "multi-user.target.wants");
    for _ in 0..2 {
        fixture.restore(&before, BOOT).unwrap();
        assert!(!introduced.try_exists().unwrap());
        assert!(runtime.try_exists().unwrap());
        assert_eq!(fixture.snapshot(BOOT), before);
    }
}

#[test]
fn mixed_memberships_are_restored_exactly_without_touching_other_services() {
    let fixture = Fixture::new();
    let persistent = fixture.link(false, "custom.target.requires");
    let runtime = fixture.link(true, "multi-user.target.wants");
    let other = persistent.with_file_name("other.service");
    symlink(fixture.directory.join("foreign.service"), &other).unwrap();
    let before = fixture.snapshot(BOOT);
    std::fs::remove_file(&persistent).unwrap();
    std::fs::remove_file(&runtime).unwrap();
    let introduced = fixture.link(true, "unexpected.target.wants");
    fixture.restore(&before, BOOT).unwrap();
    assert_eq!(fixture.snapshot(BOOT), before);
    assert!(!introduced.try_exists().unwrap());
    assert_eq!(
        std::fs::read_link(other).unwrap(),
        fixture.directory.join("foreign.service")
    );
}

#[test]
fn disabled_restore_removes_all_introduced_memberships_but_preserves_registration() {
    let fixture = Fixture::new();
    let before = fixture.snapshot(BOOT);
    let persistent = fixture.link(false, "multi-user.target.wants");
    let runtime = fixture.link(true, "custom.target.requires");
    fixture.restore(&before, BOOT).unwrap();
    assert!(!persistent.try_exists().unwrap());
    assert!(!runtime.try_exists().unwrap());
    assert_eq!(fixture.snapshot(BOOT), before);
    assert!(
        fixture
            .systemd
            .system_unit_dir
            .join(UNIT_NAME)
            .try_exists()
            .unwrap()
    );
}

#[test]
fn host_reboot_expires_runtime_memberships_and_keeps_persistent_ones() {
    for persistent in [false, true] {
        let fixture = Fixture::new();
        let runtime = fixture.link(true, "multi-user.target.wants");
        if persistent {
            fixture.link(false, "custom.target.requires");
        }
        let before = fixture.snapshot(BOOT);
        let introduced = fixture.link(false, "unexpected.target.wants");
        fixture.restore(&before, NEXT_BOOT).unwrap();
        assert!(!runtime.try_exists().unwrap());
        assert!(!introduced.try_exists().unwrap());
        let actual = fixture.snapshot(NEXT_BOOT);
        assert_eq!(actual.enabled, persistent);
        assert!(before.matches_current(&actual).unwrap());
        assert!(actual.systemd_links.unwrap().runtime.is_empty());
    }
}

#[test]
fn foreign_or_redirected_membership_blocks_all_changes() {
    for redirect in [false, true] {
        let fixture = Fixture::new();
        let runtime = fixture.link(true, "multi-user.target.wants");
        let before = fixture.snapshot(BOOT);
        std::fs::remove_file(&runtime).unwrap();
        let introduced = fixture.link(false, "unexpected.target.wants");
        if redirect {
            let parent = runtime.parent().unwrap();
            std::fs::remove_dir(parent).unwrap();
            let foreign = fixture.directory.join("foreign");
            std::fs::create_dir(&foreign).unwrap();
            symlink(&foreign, parent).unwrap();
        } else {
            std::fs::write(&runtime, "unknown membership").unwrap();
        }
        assert!(fixture.restore(&before, BOOT).is_err());
        assert!(
            introduced.try_exists().unwrap(),
            "preflight must finish before removing owned links"
        );
        if !redirect {
            assert_eq!(
                std::fs::read_to_string(runtime).unwrap(),
                "unknown membership"
            );
        }
    }
}

#[test]
fn foreign_primary_registration_blocks_restoring_owned_links() {
    let fixture = Fixture::new();
    fixture.link(true, "multi-user.target.wants");
    let before = fixture.snapshot(BOOT);
    let primary = fixture.systemd.system_unit_dir.join(UNIT_NAME);
    std::fs::remove_file(&primary).unwrap();
    std::fs::write(&primary, "foreign definition").unwrap();
    assert!(fixture.restore(&before, BOOT).is_err());
    assert_eq!(
        std::fs::read_to_string(primary).unwrap(),
        "foreign definition"
    );
}

#[test]
fn unknown_unregistered_memberships_cannot_be_captured() {
    let fixture = Fixture::new();
    fixture.link(true, "multi-user.target.wants");
    assert!(capture(&fixture.systemd, UNIT_NAME, None, BOOT).is_err());
}

#[test]
fn daemon_enablement_uses_the_same_exact_runtime_restoration() {
    let fixture = Fixture::new();
    let unit = super::super::LKIT_DAEMON_UNIT_NAME;
    let origin = fixture.directory.join(unit);
    std::fs::write(&origin, "test daemon definition").unwrap();
    symlink(&origin, fixture.systemd.system_unit_dir.join(unit)).unwrap();
    let runtime = fixture
        .systemd
        .run_systemd_dir
        .join("multi-user.target.wants");
    std::fs::create_dir(&runtime).unwrap();
    symlink(&origin, runtime.join(unit)).unwrap();
    let before = capture(&fixture.systemd, unit, Some(&origin), BOOT).unwrap();
    std::fs::remove_file(runtime.join(unit)).unwrap();
    let persistent = fixture
        .systemd
        .system_unit_dir
        .join("multi-user.target.wants");
    std::fs::create_dir(&persistent).unwrap();
    symlink(&origin, persistent.join(unit)).unwrap();
    restore(
        &fixture.systemd,
        unit,
        &origin,
        before.systemd_links.as_ref().unwrap(),
        BOOT,
    )
    .unwrap();
    assert!(!persistent.join(unit).try_exists().unwrap());
    assert!(runtime.join(unit).try_exists().unwrap());
    assert_eq!(
        capture(&fixture.systemd, unit, Some(&origin), BOOT).unwrap(),
        before
    );
}
