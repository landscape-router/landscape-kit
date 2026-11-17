use super::super::{ManagedService, ServiceManager, render_unit};
use super::*;

struct Fixture {
    directory: PathBuf,
    systemd: Systemd,
    origin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("lkit-boot-unit-{}", uuid::Uuid::now_v7()));
        let systemd = Systemd {
            systemctl: directory.join("never-execute-systemctl"),
            system_unit_dir: directory.join("units"),
            run_systemd_dir: directory.join("runtime-units"),
            pid1_is_systemd: true,
            resolv_conf: directory.join("resolv.conf"),
        };
        std::fs::create_dir_all(&systemd.system_unit_dir).unwrap();
        std::fs::create_dir_all(&systemd.run_systemd_dir).unwrap();
        let install_root = directory.join("installation");
        let origin = install_root.join("service").join(UNIT_NAME);
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        std::fs::write(&origin, render_unit(&install_root)).unwrap();
        Self {
            directory,
            systemd,
            origin,
        }
    }

    fn registration(&self) -> PathBuf {
        self.systemd.system_unit_dir.join(UNIT_NAME)
    }
    fn copy(&self) -> PathBuf {
        copy_path(&self.systemd, &self.origin).unwrap()
    }
    fn publish(&self) {
        publish(&self.systemd, &self.origin, true).unwrap();
    }
    fn assert_logical_registration(&self) {
        assert_eq!(
            self.systemd
                .query_registration(ManagedService::LandscapeRouter)
                .unwrap(),
            SystemRegistration::Symlink {
                target: self.origin.clone()
            }
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn linked_definition_and_alias_do_not_imply_startup_membership() {
    for state in ["enabled", "enabled-runtime"] {
        assert_eq!(super::super::parse_enabled_state(state), Some(true));
    }
    for state in ["linked", "linked-runtime", "alias", "disabled", "masked"] {
        assert_eq!(super::super::parse_enabled_state(state), Some(false));
    }
    assert_eq!(super::super::parse_enabled_state("unexpected"), None);
}

#[test]
fn published_unit_remains_readable_without_the_installation_filesystem() {
    let fixture = Fixture::new();
    fixture.publish();
    assert_eq!(
        std::fs::read_link(fixture.registration()).unwrap(),
        fixture.copy()
    );
    assert!(
        !fixture.copy().starts_with(&fixture.systemd.system_unit_dir),
        "same-name unit must not be treated as an alias within the unit search path"
    );
    assert!(
        std::fs::symlink_metadata(fixture.copy())
            .unwrap()
            .file_type()
            .is_file()
    );
    let source = std::fs::read_to_string(&fixture.origin).unwrap();
    assert_eq!(
        read_copy(&fixture.systemd, &fixture.copy()).unwrap().1,
        source
    );
    let loaded = std::fs::read_to_string(fixture.registration()).unwrap();
    assert!(loaded.contains(&mount_requirement(root(&fixture.origin).unwrap())));
    std::fs::rename(
        root(&fixture.origin).unwrap(),
        fixture.directory.join("unmounted-installation"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.registration()).unwrap(),
        loaded
    );
    fixture.assert_logical_registration();
}

#[test]
fn legacy_migration_preserves_persistent_and_runtime_memberships() {
    let fixture = Fixture::new();
    let legacy = std::fs::read_to_string(&fixture.origin).unwrap().replace(
        &format!("{}\n", mount_requirement(root(&fixture.origin).unwrap())),
        "",
    );
    std::fs::write(&fixture.origin, &legacy).unwrap();
    symlink(&fixture.origin, fixture.registration()).unwrap();
    let persistent = fixture
        .systemd
        .system_unit_dir
        .join("multi-user.target.wants");
    let relative = fixture
        .systemd
        .system_unit_dir
        .join("custom.target.requires");
    let runtime = fixture
        .systemd
        .run_systemd_dir
        .join("multi-user.target.wants");
    for directory in [&persistent, &relative, &runtime] {
        std::fs::create_dir(directory).unwrap();
    }
    symlink(&fixture.origin, persistent.join(UNIT_NAME)).unwrap();
    symlink(Path::new("..").join(UNIT_NAME), relative.join(UNIT_NAME)).unwrap();
    symlink(&fixture.origin, runtime.join(UNIT_NAME)).unwrap();
    assert!(needs_refresh(&fixture.systemd, &fixture.origin).unwrap());
    fixture.publish();
    for directory in [&persistent, &relative, &runtime] {
        assert_eq!(
            std::fs::read_link(directory.join(UNIT_NAME)).unwrap(),
            fixture.copy()
        );
    }
    fixture.assert_logical_registration();
    assert_eq!(std::fs::read_to_string(&fixture.origin).unwrap(), legacy);
    assert!(
        std::fs::read_to_string(fixture.copy())
            .unwrap()
            .contains(&mount_requirement(root(&fixture.origin).unwrap()))
    );
    assert!(!needs_refresh(&fixture.systemd, &fixture.origin).unwrap());
}

#[test]
fn migration_does_not_enable_a_registered_but_disabled_service() {
    let fixture = Fixture::new();
    symlink(&fixture.origin, fixture.registration()).unwrap();
    fixture.publish();
    assert!(
        !fixture
            .systemd
            .system_unit_dir
            .join("multi-user.target.wants")
            .exists()
    );
    assert_eq!(
        std::fs::read_dir(&fixture.systemd.run_systemd_dir)
            .unwrap()
            .count(),
        0
    );
    fixture.assert_logical_registration();
}

#[test]
fn source_and_boot_copy_may_differ_during_repair_and_rollback() {
    let fixture = Fixture::new();
    fixture.publish();
    let original = std::fs::read_to_string(&fixture.origin).unwrap();
    let replacement = original.replace(
        "Description=Landscape Router",
        "Description=Repaired Landscape",
    );
    std::fs::write(&fixture.origin, &replacement).unwrap();
    fixture.assert_logical_registration();
    assert!(needs_refresh(&fixture.systemd, &fixture.origin).unwrap());
    publish(&fixture.systemd, &fixture.origin, false).unwrap();
    assert_eq!(
        read_copy(&fixture.systemd, &fixture.copy()).unwrap().1,
        replacement
    );
    std::fs::write(&fixture.origin, &original).unwrap();
    fixture.assert_logical_registration();
    publish(&fixture.systemd, &fixture.origin, false).unwrap();
    assert_eq!(
        read_copy(&fixture.systemd, &fixture.copy()).unwrap().1,
        original
    );
}

#[test]
fn a_published_copy_without_registration_can_be_recovered_and_cleaned_without_its_source() {
    let fixture = Fixture::new();
    fixture.publish();
    std::fs::remove_file(fixture.registration()).unwrap();
    fixture.publish();
    fixture.assert_logical_registration();
    // Disable can remove the registration; failed install cleanup can remove
    // the installation directory before restoring the original Missing state.
    std::fs::remove_file(fixture.registration()).unwrap();
    std::fs::remove_dir_all(root(&fixture.origin).unwrap()).unwrap();
    unregister(&fixture.systemd, &fixture.origin).unwrap();
    assert!(!fixture.copy().exists());
    unregister(&fixture.systemd, &fixture.origin).unwrap();
}

#[test]
fn changed_boot_copy_is_not_treated_as_owned_or_overwritten() {
    let fixture = Fixture::new();
    fixture.publish();
    let changed = std::fs::read_to_string(fixture.copy())
        .unwrap()
        .replace("Description=Landscape Router", "Description=User Change");
    std::fs::write(fixture.copy(), &changed).unwrap();
    assert!(
        fixture
            .systemd
            .query_registration(ManagedService::LandscapeRouter)
            .is_err()
    );
    assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
    assert!(unregister(&fixture.systemd, &fixture.origin).is_err());
    assert_eq!(std::fs::read_to_string(fixture.copy()).unwrap(), changed);
    assert_eq!(
        std::fs::read_link(fixture.registration()).unwrap(),
        fixture.copy()
    );
}

#[test]
fn foreign_registration_and_unknown_orphan_copy_are_preserved() {
    let fixture = Fixture::new();
    std::fs::write(fixture.registration(), "foreign unit").unwrap();
    assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
    assert!(!directory(&fixture.systemd).unwrap().exists());
    assert_eq!(
        std::fs::read_to_string(fixture.registration()).unwrap(),
        "foreign unit"
    );
    std::fs::remove_file(fixture.registration()).unwrap();
    std::fs::create_dir_all(fixture.copy().parent().unwrap()).unwrap();
    std::fs::write(fixture.copy(), "unknown copy").unwrap();
    assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
    assert!(unregister(&fixture.systemd, &fixture.origin).is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.copy()).unwrap(),
        "unknown copy"
    );
    assert!(!fixture.registration().exists());
}

#[test]
fn foreign_or_redirected_startup_membership_blocks_publication_before_mutation() {
    for redirected in [false, true] {
        let fixture = Fixture::new();
        symlink(&fixture.origin, fixture.registration()).unwrap();
        let membership_directory = fixture
            .systemd
            .system_unit_dir
            .join("multi-user.target.wants");
        if redirected {
            let outside = fixture.directory.join("foreign-wants");
            std::fs::create_dir(&outside).unwrap();
            symlink(&fixture.origin, outside.join(UNIT_NAME)).unwrap();
            symlink(&outside, &membership_directory).unwrap();
        } else {
            std::fs::create_dir(&membership_directory).unwrap();
            symlink(
                fixture.directory.join("foreign.service"),
                membership_directory.join(UNIT_NAME),
            )
            .unwrap();
        }
        assert!(needs_refresh(&fixture.systemd, &fixture.origin).is_err());
        assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
        assert!(!directory(&fixture.systemd).unwrap().exists());
        assert_eq!(
            std::fs::read_link(fixture.registration()).unwrap(),
            fixture.origin
        );
    }
}

#[test]
fn redirected_boot_directory_is_never_used_for_publication() {
    let fixture = Fixture::new();
    let outside = fixture.directory.join("foreign-storage");
    std::fs::create_dir(&outside).unwrap();
    symlink(&outside, directory(&fixture.systemd).unwrap()).unwrap();
    assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    assert!(!fixture.registration().exists());
}

#[test]
fn legacy_preflight_rejects_unknown_or_redirected_boot_storage_without_changes() {
    for condition in [
        "unknown-copy",
        "redirected-base",
        "redirected-origin",
        "file-base",
        "file-origin",
    ] {
        let fixture = Fixture::new();
        let original = std::fs::read(&fixture.origin).unwrap();
        symlink(&fixture.origin, fixture.registration()).unwrap();
        let base = directory(&fixture.systemd).unwrap();
        let copy = fixture.copy();
        let outside = fixture.directory.join("outside-storage");
        std::fs::create_dir(&outside).unwrap();
        match condition {
            "unknown-copy" => {
                std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
                std::fs::write(&copy, "unknown definition").unwrap();
            }
            "redirected-base" => symlink(&outside, &base).unwrap(),
            "redirected-origin" => {
                std::fs::create_dir(&base).unwrap();
                symlink(&outside, copy.parent().unwrap()).unwrap();
            }
            "file-base" => std::fs::write(&base, "unknown base").unwrap(),
            "file-origin" => {
                std::fs::create_dir(&base).unwrap();
                std::fs::write(copy.parent().unwrap(), "unknown origin").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            needs_refresh(&fixture.systemd, &fixture.origin).is_err(),
            "{condition}"
        );
        assert!(
            publish(&fixture.systemd, &fixture.origin, false).is_err(),
            "{condition}"
        );
        assert_eq!(
            std::fs::read_link(fixture.registration()).unwrap(),
            fixture.origin
        );
        assert_eq!(std::fs::read(&fixture.origin).unwrap(), original);
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        match condition {
            "unknown-copy" => {
                assert_eq!(std::fs::read_to_string(copy).unwrap(), "unknown definition")
            }
            "redirected-base" => assert_eq!(std::fs::read_link(base).unwrap(), outside),
            "redirected-origin" => {
                assert_eq!(std::fs::read_link(copy.parent().unwrap()).unwrap(), outside)
            }
            "file-base" => assert_eq!(std::fs::read_to_string(base).unwrap(), "unknown base"),
            "file-origin" => assert_eq!(
                std::fs::read_to_string(copy.parent().unwrap()).unwrap(),
                "unknown origin"
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn legacy_preflight_rejects_redirected_or_invalid_source_without_publication() {
    for redirected in [false, true] {
        let fixture = Fixture::new();
        symlink(&fixture.origin, fixture.registration()).unwrap();
        let source = if redirected {
            let actual = fixture.origin.with_file_name("actual.service");
            std::fs::rename(&fixture.origin, &actual).unwrap();
            symlink(&actual, &fixture.origin).unwrap();
            actual
        } else {
            let changed = std::fs::read_to_string(&fixture.origin)
                .unwrap()
                .replace("current/landscape-webserver", "foreign/webserver");
            std::fs::write(&fixture.origin, changed).unwrap();
            fixture.origin.clone()
        };
        let original = std::fs::read(&source).unwrap();
        assert!(needs_refresh(&fixture.systemd, &fixture.origin).is_err());
        assert!(publish(&fixture.systemd, &fixture.origin, false).is_err());
        assert!(!directory(&fixture.systemd).unwrap().exists());
        assert_eq!(
            std::fs::read_link(fixture.registration()).unwrap(),
            fixture.origin
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
    }
}

#[test]
fn copy_identity_cannot_be_transplanted_to_another_origin_directory() {
    let fixture = Fixture::new();
    fixture.publish();
    let foreign = directory(&fixture.systemd)
        .unwrap()
        .join("other-origin")
        .join(UNIT_NAME);
    std::fs::create_dir(foreign.parent().unwrap()).unwrap();
    std::fs::copy(fixture.copy(), &foreign).unwrap();
    std::fs::remove_file(fixture.registration()).unwrap();
    symlink(&foreign, fixture.registration()).unwrap();
    assert!(
        fixture
            .systemd
            .query_registration(ManagedService::LandscapeRouter)
            .is_err()
    );
    assert!(publish(&fixture.systemd, &fixture.origin, true).is_err());
    assert!(unregister(&fixture.systemd, &fixture.origin).is_err());
}
